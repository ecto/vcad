//! Frenet frame computation for orienting profiles along curves.

use vcad_kernel_geom::Curve3d;
use vcad_kernel_math::{Dir3, Point2, Point3, Vec3};

/// A Frenet frame at a point on a curve.
///
/// The Frenet frame provides an orthonormal basis for orienting a 2D profile
/// in 3D space as it moves along a curve. It consists of:
/// - **Tangent**: The direction along the curve
/// - **Normal**: The direction toward the center of curvature
/// - **Binormal**: Tangent × Normal (perpendicular to both)
///
/// For straight paths where normal is undefined, a rotation-minimizing frame
/// is computed instead.
#[derive(Debug, Clone)]
pub struct FrenetFrame {
    /// Position on the curve.
    pub position: Point3,
    /// Unit tangent vector (along the curve).
    pub tangent: Dir3,
    /// Unit normal vector (toward curvature center).
    pub normal: Dir3,
    /// Unit binormal vector (tangent × normal).
    pub binormal: Dir3,
}

impl FrenetFrame {
    /// Compute a Frenet frame at parameter `t` on the given curve.
    ///
    /// Uses finite differences to approximate the tangent and second derivative.
    /// For straight paths (zero curvature), falls back to an arbitrary but
    /// consistent normal direction.
    pub fn from_curve(curve: &dyn Curve3d, t: f64) -> Self {
        let (t_min, t_max) = curve.domain();
        let dt = (t_max - t_min) * 1e-6;

        let position = curve.evaluate(t);
        let tangent_vec = curve.tangent(t);
        let tangent_len = tangent_vec.norm();

        if tangent_len < 1e-12 {
            // Degenerate point - use default frame
            return Self::default_at(position);
        }

        let tangent = Dir3::new_normalize(tangent_vec);

        // Compute second derivative via finite differences for normal direction
        let t_prev = (t - dt).max(t_min);
        let t_next = (t + dt).min(t_max);

        let tan_prev = curve.tangent(t_prev);
        let tan_next = curve.tangent(t_next);

        // Second derivative approximation
        let d2 = (tan_next - tan_prev) / ((t_next - t_prev).max(1e-12));

        // Normal is the component of d2 perpendicular to tangent
        let d2_parallel = d2.dot(tangent.as_ref()) * tangent.as_ref();
        let d2_perp = d2 - d2_parallel;

        if d2_perp.norm() < 1e-12 {
            // Straight line or inflection point - use arbitrary perpendicular
            Self::with_arbitrary_normal(position, tangent)
        } else {
            let normal = Dir3::new_normalize(d2_perp);
            let binormal = Dir3::new_normalize(tangent.as_ref().cross(normal.as_ref()));
            Self {
                position,
                tangent,
                normal,
                binormal,
            }
        }
    }

    /// Create a frame with an arbitrary but consistent normal direction.
    ///
    /// Used when the curve has zero curvature (straight line).
    fn with_arbitrary_normal(position: Point3, tangent: Dir3) -> Self {
        // Choose an arbitrary vector not parallel to tangent
        let arbitrary = if tangent.as_ref().x.abs() < 0.9 {
            Vec3::x()
        } else {
            Vec3::y()
        };

        // Compute perpendicular vector
        let normal = Dir3::new_normalize(arbitrary.cross(tangent.as_ref()));
        let binormal = Dir3::new_normalize(tangent.as_ref().cross(normal.as_ref()));

        Self {
            position,
            tangent,
            normal,
            binormal,
        }
    }

    /// Create a default frame at origin with Z tangent.
    fn default_at(position: Point3) -> Self {
        Self {
            position,
            tangent: Dir3::new_normalize(Vec3::z()),
            normal: Dir3::new_normalize(Vec3::x()),
            binormal: Dir3::new_normalize(Vec3::y()),
        }
    }

    /// Apply a twist rotation around the tangent axis.
    ///
    /// Rotates the normal and binormal by the given angle (in radians)
    /// around the tangent vector.
    pub fn with_twist(&self, angle: f64) -> Self {
        if angle.abs() < 1e-12 {
            return self.clone();
        }

        let (sin_a, cos_a) = angle.sin_cos();

        // Rotate normal and binormal around tangent
        let new_normal = cos_a * self.normal.as_ref() + sin_a * self.binormal.as_ref();
        let new_binormal = -sin_a * self.normal.as_ref() + cos_a * self.binormal.as_ref();

        Self {
            position: self.position,
            tangent: self.tangent,
            normal: Dir3::new_normalize(new_normal),
            binormal: Dir3::new_normalize(new_binormal),
        }
    }

    /// Transform a 2D point from profile coordinates to 3D world coordinates.
    ///
    /// The profile is assumed to be in the XY plane, with X mapping to normal
    /// and Y mapping to binormal.
    pub fn transform_point(&self, p: Point2) -> Point3 {
        self.position + p.x * self.normal.as_ref() + p.y * self.binormal.as_ref()
    }

    /// Transform a 2D point with optional scale factor.
    pub fn transform_point_scaled(&self, p: Point2, scale: f64) -> Point3 {
        self.position + scale * (p.x * self.normal.as_ref() + p.y * self.binormal.as_ref())
    }

    /// Interpolate between two frames.
    ///
    /// Creates a smooth transition between frames at `t=0` (self) and `t=1` (other).
    pub fn lerp(&self, other: &FrenetFrame, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);

        // Interpolate position
        let position = Point3::new(
            self.position.x + t * (other.position.x - self.position.x),
            self.position.y + t * (other.position.y - self.position.y),
            self.position.z + t * (other.position.z - self.position.z),
        );

        // Interpolate directions (simple lerp + normalize)
        let tangent = Self::lerp_dir(&self.tangent, &other.tangent, t);
        let normal = Self::lerp_dir(&self.normal, &other.normal, t);
        let binormal = Dir3::new_normalize(tangent.as_ref().cross(normal.as_ref()));

        Self {
            position,
            tangent,
            normal,
            binormal,
        }
    }

    fn lerp_dir(a: &Dir3, b: &Dir3, t: f64) -> Dir3 {
        let v = (1.0 - t) * a.as_ref() + t * b.as_ref();
        if v.norm() < 1e-12 {
            *a
        } else {
            Dir3::new_normalize(v)
        }
    }
}

/// Compute a sequence of rotation-minimizing frames along a curve.
///
/// This produces smoother results than independent Frenet frames by
/// propagating the normal direction to minimize rotation.
pub fn rotation_minimizing_frames(curve: &dyn Curve3d, n_samples: usize) -> Vec<FrenetFrame> {
    if n_samples < 2 {
        return vec![];
    }

    let (t_min, t_max) = curve.domain();
    let dt = (t_max - t_min) / (n_samples - 1) as f64;

    let mut frames = Vec::with_capacity(n_samples);

    // First frame: use standard computation
    let first = FrenetFrame::from_curve(curve, t_min);
    frames.push(first);

    // Propagate frames using double reflection method (rotation minimizing)
    for i in 1..n_samples {
        let t = t_min + i as f64 * dt;

        let prev = &frames[i - 1];

        let xi = curve.evaluate(t);
        let xi_prev = prev.position;

        // Vector from previous to current position
        let v1 = xi - xi_prev;
        let c1 = v1.dot(v1);

        if c1 < 1e-24 {
            // Coincident points - copy previous frame
            frames.push(FrenetFrame {
                position: xi,
                ..prev.clone()
            });
            continue;
        }

        // Reflect previous tangent and normal
        let ri_l = prev.normal.as_ref() - (2.0 / c1) * v1.dot(prev.normal.as_ref()) * v1;
        let ti_l = prev.tangent.as_ref() - (2.0 / c1) * v1.dot(prev.tangent.as_ref()) * v1;

        // Get current tangent
        let ti = curve.tangent(t);
        if ti.norm() < 1e-12 {
            frames.push(FrenetFrame {
                position: xi,
                ..prev.clone()
            });
            continue;
        }
        let ti = Dir3::new_normalize(ti);

        // Second reflection to align with actual tangent
        let v2 = ti.as_ref() - ti_l;
        let c2 = v2.dot(v2);

        let ri = if c2 < 1e-24 {
            ri_l
        } else {
            ri_l - (2.0 / c2) * v2.dot(ri_l) * v2
        };

        let normal = Dir3::new_normalize(ri);
        let binormal = Dir3::new_normalize(ti.as_ref().cross(normal.as_ref()));

        frames.push(FrenetFrame {
            position: xi,
            tangent: ti,
            normal,
            binormal,
        });
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;
    use vcad_kernel_geom::{Circle3d, Line3d};

    #[test]
    fn test_frenet_frame_line() {
        let line = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));
        let frame = FrenetFrame::from_curve(&line, 0.5);

        // Tangent should be +Z
        assert!((frame.tangent.as_ref().z - 1.0).abs() < 1e-6);
        // Normal and binormal should be perpendicular to tangent
        assert!(frame.tangent.as_ref().dot(frame.normal.as_ref()).abs() < 1e-6);
        assert!(frame.tangent.as_ref().dot(frame.binormal.as_ref()).abs() < 1e-6);
    }

    #[test]
    fn test_frenet_frame_circle() {
        let circle = Circle3d::new(Point3::origin(), 10.0);
        let frame = FrenetFrame::from_curve(&circle, 0.0);

        // At t=0, position is (10, 0, 0)
        assert!((frame.position.x - 10.0).abs() < 0.1);
        // Tangent should be in +Y direction
        assert!(frame.tangent.as_ref().y > 0.5);
        // Normal should point toward center (-X direction)
        assert!(frame.normal.as_ref().x < -0.5);
    }

    #[test]
    fn test_transform_point() {
        let frame = FrenetFrame {
            position: Point3::new(10.0, 0.0, 0.0),
            tangent: Dir3::new_normalize(Vec3::z()),
            normal: Dir3::new_normalize(Vec3::x()),
            binormal: Dir3::new_normalize(Vec3::y()),
        };

        let p2d = Point2::new(5.0, 3.0);
        let p3d = frame.transform_point(p2d);

        // x should be position.x + 5 (normal direction)
        assert!((p3d.x - 15.0).abs() < 1e-6);
        // y should be position.y + 3 (binormal direction)
        assert!((p3d.y - 3.0).abs() < 1e-6);
        // z should be position.z (no tangent component)
        assert!(p3d.z.abs() < 1e-6);
    }

    #[test]
    fn test_with_twist() {
        let frame = FrenetFrame {
            position: Point3::origin(),
            tangent: Dir3::new_normalize(Vec3::z()),
            normal: Dir3::new_normalize(Vec3::x()),
            binormal: Dir3::new_normalize(Vec3::y()),
        };

        let twisted = frame.with_twist(PI / 2.0);

        // After 90° twist, normal should be in +Y
        assert!((twisted.normal.as_ref().y - 1.0).abs() < 1e-6);
        // Binormal should be in -X
        assert!((twisted.binormal.as_ref().x + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_rotation_minimizing_frames() {
        let line = Line3d::from_points(Point3::origin(), Point3::new(0.0, 0.0, 10.0));
        let frames = rotation_minimizing_frames(&line, 5);

        assert_eq!(frames.len(), 5);

        // All normals should be consistent (not flipping)
        for i in 1..frames.len() {
            let dot = frames[i].normal.as_ref().dot(frames[0].normal.as_ref());
            assert!(dot > 0.9, "normal flipped at frame {i}");
        }
    }

    #[test]
    fn test_lerp() {
        let frame1 = FrenetFrame {
            position: Point3::origin(),
            tangent: Dir3::new_normalize(Vec3::z()),
            normal: Dir3::new_normalize(Vec3::x()),
            binormal: Dir3::new_normalize(Vec3::y()),
        };

        let frame2 = FrenetFrame {
            position: Point3::new(10.0, 0.0, 0.0),
            tangent: Dir3::new_normalize(Vec3::z()),
            normal: Dir3::new_normalize(Vec3::x()),
            binormal: Dir3::new_normalize(Vec3::y()),
        };

        let mid = frame1.lerp(&frame2, 0.5);
        assert!((mid.position.x - 5.0).abs() < 1e-6);
    }
}
