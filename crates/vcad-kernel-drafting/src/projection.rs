//! 3D to 2D orthographic and isometric projection.
//!
//! Provides view matrix generation and point projection for creating
//! 2D technical drawings from 3D geometry.

use vcad_kernel_math::{Point2, Point3, Vec3};

use crate::types::ViewDirection;

/// A 4x4 view matrix for orthographic projection.
///
/// Transforms 3D world coordinates to view coordinates where:
/// - X is the horizontal axis of the drawing (viewer's right)
/// - Y is the vertical axis of the drawing (viewer's up)
/// - Z is depth toward the viewer (larger = closer; used for hidden
///   line removal)
///
/// The basis is right-handed as seen on paper: `right × up` points out of
/// the drawing, toward the viewer (`forward`). See the `handedness` test.
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
    /// is the distance toward the viewer (larger = closer; used for hidden
    /// line removal).
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

        // Front view: camera at -Y looking along +Y, so world +X is the
        // viewer's right and world +Z is up.
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        assert!((p2.x - 1.0).abs() < 1e-10, "X should be 1.0, got {}", p2.x);
        assert!((p2.y - 3.0).abs() < 1e-10, "Y should be 3.0, got {}", p2.y);
        // Depth increases toward the viewer (-Y side).
        assert!(
            (depth - (-2.0)).abs() < 1e-10,
            "depth should be -2.0, got {}",
            depth
        );
    }

    #[test]
    fn test_top_view_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::Top);

        // Top view: camera above looking down, world +X right, +Y up on paper.
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        assert!((p2.x - 1.0).abs() < 1e-10, "X should be 1.0, got {}", p2.x);
        assert!((p2.y - 2.0).abs() < 1e-10, "Y should be 2.0, got {}", p2.y);
        // Depth increases toward the viewer (+Z).
        assert!(
            (depth - 3.0).abs() < 1e-10,
            "depth should be 3.0, got {}",
            depth
        );
    }

    #[test]
    fn test_right_view_projection() {
        let view = ViewMatrix::from_view_direction(ViewDirection::Right);

        // Right view: camera at +X looking along -X; world +Y (the part's
        // back) is the viewer's right, +Z is up.
        let p = Point3::new(1.0, 2.0, 3.0);
        let (p2, depth) = view.project(p);

        assert!((p2.x - 2.0).abs() < 1e-10, "X should be 2.0, got {}", p2.x);
        assert!((p2.y - 3.0).abs() < 1e-10, "Y should be 3.0, got {}", p2.y);
        // Depth increases toward the viewer (+X).
        assert!(
            (depth - 1.0).abs() < 1e-10,
            "depth should be 1.0, got {}",
            depth
        );
    }

    /// Pins the handedness of every view basis (the drafting sibling of
    /// vcad-render's `view_basis_handedness_is_pinned`, PR #728).
    ///
    /// On paper, `right × up` must point out of the drawing toward the
    /// viewer — i.e. along `forward` (scene → viewer). A basis where it
    /// points the other way draws a mirror image: invisible on a symmetric
    /// part, unusable on a shop drawing. Every view was mirrored until
    /// 2026-07.
    #[test]
    fn handedness_is_pinned_for_every_view() {
        for view_dir in [
            ViewDirection::Front,
            ViewDirection::Back,
            ViewDirection::Top,
            ViewDirection::Bottom,
            ViewDirection::Right,
            ViewDirection::Left,
            ViewDirection::ISOMETRIC_STANDARD,
            ViewDirection::DIMETRIC,
            // Degenerate-fallback path: looking straight down/up.
            ViewDirection::Isometric {
                azimuth: 0.0,
                elevation: std::f64::consts::FRAC_PI_2,
            },
            ViewDirection::Isometric {
                azimuth: 0.0,
                elevation: -std::f64::consts::FRAC_PI_2,
            },
        ] {
            let view = ViewMatrix::from_view_direction(view_dir);
            let out_of_screen = view.right.cross(view.up);
            let dot = out_of_screen.dot(view.forward);
            assert!(
                (dot - 1.0).abs() < 1e-9,
                "{view_dir:?}: right x up . forward = {dot}, expected 1 (mirrored basis?)",
            );
        }
    }

    /// The six principal views must match third-angle convention exactly:
    /// pinned world axes on the paper, not just "some orthonormal basis".
    #[test]
    fn principal_view_axes_are_pinned() {
        let cases: &[(ViewDirection, [f64; 3], [f64; 3])] = &[
            // (view, world axis on drawing +X, world axis on drawing +Y)
            (ViewDirection::Front, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            (ViewDirection::Back, [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            (ViewDirection::Top, [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            (ViewDirection::Bottom, [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
            (ViewDirection::Right, [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            (ViewDirection::Left, [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
        ];
        for &(dir, want_right, want_up) in cases {
            let view = ViewMatrix::from_view_direction(dir);
            for (got, want, name) in [(view.right, want_right, "right"), (view.up, want_up, "up")] {
                assert!(
                    (got.x - want[0]).abs() < 1e-12
                        && (got.y - want[1]).abs() < 1e-12
                        && (got.z - want[2]).abs() < 1e-12,
                    "{dir:?}: {name} = ({}, {}, {}), expected {want:?}",
                    got.x,
                    got.y,
                    got.z,
                );
            }
        }
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

        // Front view: X stays X, Z becomes Y.
        assert!((p2.x - 5.0).abs() < 1e-10, "X should be 5.0, got {}", p2.x);
        assert!(
            (p2.y - 10.0).abs() < 1e-10,
            "Y should be 10.0, got {}",
            p2.y
        );
    }
}
