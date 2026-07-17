//! 3D to 2D orthographic and isometric projection.
//!
//! Provides view matrix generation and point projection for creating
//! 2D technical drawings from 3D geometry.

use vcad_kernel_math::{Point2, Point3, Vec3};

use crate::types::ViewDirection;

/// A 4x4 view matrix for orthographic projection.
///
/// Transforms 3D world coordinates to view coordinates where:
/// - X is the horizontal axis of the drawing
/// - Y is the vertical axis of the drawing
/// - Z is depth (used for hidden line removal)
#[derive(Debug, Clone, Copy)]
pub struct ViewMatrix {
    /// Row 0: right vector (X axis in view space)
    pub right: Vec3,
    /// Row 1: up vector (Y axis in view space)
    pub up: Vec3,
    /// Row 2: forward vector (Z axis in view space, toward viewer)
    pub forward: Vec3,
}

impl ViewMatrix {
    /// Create a view matrix for the given view direction.
    pub fn from_view_direction(dir: ViewDirection) -> Self {
        let forward = dir.view_vector();
        let world_up = dir.up_vector();

        // Compute right vector (X axis in view space)
        let right = world_up.cross(forward);
        let right_len = right.norm();

        // Handle degenerate case where view direction is parallel to up
        let right = if right_len < 1e-10 {
            // Fall back to using world X as reference
            let alt_up = Vec3::new(1.0, 0.0, 0.0);
            alt_up.cross(forward).normalize()
        } else {
            right / right_len
        };

        // Recompute up to ensure orthogonality
        let up = forward.cross(right).normalize();

        Self { right, up, forward }
    }

    /// Project a 3D point to 2D view coordinates.
    ///
    /// Returns (x, y, depth) where x/y are the 2D coordinates and depth
    /// is the distance along the view direction (used for hidden line removal).
    pub fn project(&self, p: Point3) -> (Point2, f64) {
        let v = Vec3::new(p.x, p.y, p.z);
        let x = v.dot(self.right);
        let y = v.dot(self.up);
        let depth = v.dot(self.forward);
        (Point2::new(x, y), depth)
    }

    /// Project a 3D point to 2D, returning only the 2D coordinates.
    pub fn project_point(&self, p: Point3) -> Point2 {
        let v = Vec3::new(p.x, p.y, p.z);
        Point2::new(v.dot(self.right), v.dot(self.up))
    }

    /// Get the depth (distance along view direction) for a 3D point.
    pub fn depth(&self, p: Point3) -> f64 {
        let v = Vec3::new(p.x, p.y, p.z);
        v.dot(self.forward)
    }

    /// Transform a 3D vector to view space (ignoring translation).
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        Vec3::new(v.dot(self.right), v.dot(self.up), v.dot(self.forward))
    }
}

/// Project a single 3D point to 2D using the given view direction.
pub fn project_point(p: Point3, view: ViewDirection) -> Point2 {
    ViewMatrix::from_view_direction(view).project_point(p)
}

/// Project a single 3D point to 2D with depth information.
pub fn project_point_with_depth(p: Point3, view: ViewDirection) -> (Point2, f64) {
    ViewMatrix::from_view_direction(view).project(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_front_view_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::Front);

        // Front view: looking along +Y
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        // Z should map to drawing Y (height)
        assert!((p2.y - 3.0).abs() < 1e-10, "Y should be 3.0, got {}", p2.y);
        // Y is depth (distance along view direction)
        assert!(
            (depth - 2.0).abs() < 1e-10,
            "depth should be 2.0, got {}",
            depth
        );
        // X maps to drawing X (may be negated depending on coordinate system)
        assert!(
            (p2.x.abs() - 1.0).abs() < 1e-10,
            "X magnitude should be 1.0, got {}",
            p2.x
        );
    }

    #[test]
    fn test_top_view_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::Top);

        // Top view: looking along -Z
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        // Y maps to drawing Y
        assert!((p2.y - 2.0).abs() < 1e-10, "Y should be 2.0, got {}", p2.y);
        // Z is depth (negative because looking down)
        assert!(
            (depth - (-3.0)).abs() < 1e-10,
            "depth should be -3.0, got {}",
            depth
        );
        // X maps to drawing X (may be negated)
        assert!(
            (p2.x.abs() - 1.0).abs() < 1e-10,
            "X magnitude should be 1.0, got {}",
            p2.x
        );
    }

    #[test]
    fn test_right_view_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::Right);

        // Right view: looking along +X
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        // Z maps to drawing Y
        assert!((p2.y - 3.0).abs() < 1e-10, "Y should be 3.0, got {}", p2.y);
        // X is depth
        assert!(
            (depth - 1.0).abs() < 1e-10,
            "depth should be 1.0, got {}",
            depth
        );
        // Y maps to drawing X
        assert!(
            (p2.x.abs() - 2.0).abs() < 1e-10,
            "X magnitude should be 2.0, got {}",
            p2.x
        );
    }

    #[test]
    fn test_isometric_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::ISOMETRIC_STANDARD);

        // In isometric, the origin should project to origin
        let origin = Point3::new(0.0, 0.0, 0.0);
        let p2 = view.project_point(origin);
        assert!(p2.x.abs() < 1e-10);
        assert!(p2.y.abs() < 1e-10);

        // A point along +Z should project upward
        let up_point = Point3::new(0.0, 0.0, 10.0);
        let p2 = view.project_point(up_point);
        assert!(p2.y > 0.0, "Z+ should project to positive Y");
    }

    #[test]
    fn test_view_matrix_orthogonality() {
        for view_dir in [
            ViewDirection::Front,
            ViewDirection::Back,
            ViewDirection::Top,
            ViewDirection::Bottom,
            ViewDirection::Right,
            ViewDirection::Left,
            ViewDirection::ISOMETRIC_STANDARD,
        ] {
            let view = ViewMatrix::from_view_direction(view_dir);

            // All axes should be unit length
            assert!((view.right.norm() - 1.0).abs() < 1e-10);
            assert!((view.up.norm() - 1.0).abs() < 1e-10);
            assert!((view.forward.norm() - 1.0).abs() < 1e-10);

            // All axes should be orthogonal
            assert!(view.right.dot(view.up).abs() < 1e-10);
            assert!(view.right.dot(view.forward).abs() < 1e-10);
            assert!(view.up.dot(view.forward).abs() < 1e-10);
        }
    }

    #[test]
    fn test_project_point_convenience() {
        let p = Point3::new(5.0, 0.0, 10.0);
        let p2 = project_point(p, ViewDirection::Front);

        // Front view: Z becomes Y
        assert!(
            (p2.y - 10.0).abs() < 1e-10,
            "Y should be 10.0, got {}",
            p2.y
        );
        // X maps to drawing X (may be negated)
        assert!(
            (p2.x.abs() - 5.0).abs() < 1e-10,
            "X magnitude should be 5.0, got {}",
            p2.x
        );
    }
}
