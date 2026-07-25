//! Exact geometric predicates using adaptive-precision arithmetic.
//!
//! This module re-exports the robust geometric predicates from `tang`
//! (Shewchuk's algorithms via the `robust` crate, behind tang's `exact`
//! feature). These predicates use adaptive precision: fast when possible,
//! exact when needed. They eliminate the need for epsilon-based tolerance
//! tuning that doesn't scale with geometry size.
//!
//! # Primary Predicates
//!
//! - [`orient2d`]: Determines which side of a line a point lies on (2D)
//! - [`orient3d`]: Determines which side of a plane a point lies on (3D)
//! - [`incircle`]: Determines if a point is inside/outside a circle (2D)
//! - [`insphere`]: Determines if a point is inside/outside a sphere (3D)
//!
//! # Derived Predicates
//!
//! - [`point_on_segment_2d`]: Test if a point lies on a line segment
//! - [`point_on_plane`]: Test if a point lies on a plane defined by three points
//!
//! Two derived predicates are kept local rather than re-exported because
//! their semantics differ from tang's versions of the same name:
//! [`point_on_segment_2d`] (tang's accepts any collinear point when the
//! segment is degenerate) and [`point_side_of_line`] (tang's returns `None`
//! near endpoints instead of on collinearity).

pub use tang::predicates::{
    are_collinear_2d, are_coplanar, incircle, insphere, orient2d, orient3d, point_on_plane, Sign,
};

use crate::Point2;

/// Test if point `p` lies on the line segment from `a` to `b`.
///
/// Returns true if `p` is collinear with `a` and `b`, and lies between them
/// (inclusive of endpoints).
///
/// # Example
///
/// ```
/// use vcad_kernel_math::{Point2, predicates::point_on_segment_2d};
///
/// let a = Point2::new(0.0, 0.0);
/// let b = Point2::new(2.0, 0.0);
/// let p = Point2::new(1.0, 0.0);
///
/// assert!(point_on_segment_2d(&p, &a, &b)); // p is on the segment
/// ```
pub fn point_on_segment_2d(p: &Point2, a: &Point2, b: &Point2) -> bool {
    // First check collinearity using exact predicate
    if !orient2d(a, b, p).is_zero() {
        return false;
    }

    // Now we know p is collinear with a and b.
    // Since we already know collinearity, a bounding box check suffices;
    // it also handles a degenerate segment (a == b) correctly.
    let min_x = a.x.min(b.x);
    let max_x = a.x.max(b.x);
    let min_y = a.y.min(b.y);
    let max_y = a.y.max(b.y);

    p.x >= min_x && p.x <= max_x && p.y >= min_y && p.y <= max_y
}

/// Determine which side of a line segment a point is on, with segment endpoint handling.
///
/// This is useful for ray casting algorithms. Returns:
/// - `Some(Sign::Positive)`: point is strictly left of the line
/// - `Some(Sign::Negative)`: point is strictly right of the line
/// - `None`: point is on the line (collinear)
#[inline]
pub fn point_side_of_line(p: &Point2, a: &Point2, b: &Point2) -> Option<Sign> {
    let sign = orient2d(a, b, p);
    if sign.is_zero() {
        None
    } else {
        Some(sign)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point3;

    // ==========================================================================
    // orient2d tests
    // ==========================================================================

    #[test]
    fn test_orient2d_ccw() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.5, 1.0);
        assert_eq!(orient2d(&a, &b, &c), Sign::Positive);
    }

    #[test]
    fn test_orient2d_cw() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.5, -1.0);
        assert_eq!(orient2d(&a, &b, &c), Sign::Negative);
    }

    #[test]
    fn test_orient2d_collinear() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let c = Point2::new(1.0, 0.0);
        assert_eq!(orient2d(&a, &b, &c), Sign::Zero);
    }

    #[test]
    fn test_orient2d_near_collinear() {
        // Points that are very close to collinear but not exactly
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.5, 1e-15);
        // The exact predicate should detect this tiny offset
        assert_eq!(orient2d(&a, &b, &c), Sign::Positive);
    }

    // ==========================================================================
    // orient3d tests
    // ==========================================================================

    #[test]
    fn test_orient3d_above_plane() {
        // Triangle in XY plane, CCW when viewed from +Z
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.0, 0.0, 1.0);
        // d is at +Z (above the plane), so orient3d returns Negative
        assert_eq!(orient3d(&a, &b, &c, &d), Sign::Negative);
    }

    #[test]
    fn test_orient3d_below_plane() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.0, 0.0, -1.0);
        // d is at -Z (below the plane), so orient3d returns Positive
        assert_eq!(orient3d(&a, &b, &c, &d), Sign::Positive);
    }

    #[test]
    fn test_orient3d_coplanar() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.5, 0.5, 0.0);
        assert_eq!(orient3d(&a, &b, &c, &d), Sign::Zero);
    }

    #[test]
    fn test_orient3d_near_coplanar() {
        // Point very slightly above the XY plane
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(0.5, 0.5, 1e-15);
        // The exact predicate should detect this tiny offset
        // d is slightly above, so orient3d returns Negative
        assert_eq!(orient3d(&a, &b, &c, &d), Sign::Negative);
    }

    // ==========================================================================
    // incircle tests
    // ==========================================================================

    #[test]
    fn test_incircle_inside() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.5, 0.866025403784); // equilateral triangle
        let d = Point2::new(0.5, 0.3);
        assert_eq!(incircle(&a, &b, &c, &d), Sign::Positive);
    }

    #[test]
    fn test_incircle_outside() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        let c = Point2::new(0.5, 0.866025403784);
        let d = Point2::new(2.0, 2.0);
        assert_eq!(incircle(&a, &b, &c, &d), Sign::Negative);
    }

    // ==========================================================================
    // insphere tests
    // ==========================================================================

    #[test]
    fn test_insphere_inside() {
        // Regular tetrahedron centered at origin
        let a = Point3::new(1.0, 1.0, 1.0);
        let b = Point3::new(1.0, -1.0, -1.0);
        let c = Point3::new(-1.0, 1.0, -1.0);
        let d = Point3::new(-1.0, -1.0, 1.0);
        let e = Point3::new(0.0, 0.0, 0.0); // center
        assert_eq!(insphere(&a, &b, &c, &d, &e), Sign::Positive);
    }

    #[test]
    fn test_insphere_outside() {
        let a = Point3::new(1.0, 1.0, 1.0);
        let b = Point3::new(1.0, -1.0, -1.0);
        let c = Point3::new(-1.0, 1.0, -1.0);
        let d = Point3::new(-1.0, -1.0, 1.0);
        let e = Point3::new(10.0, 10.0, 10.0);
        assert_eq!(insphere(&a, &b, &c, &d, &e), Sign::Negative);
    }

    // ==========================================================================
    // Derived predicate tests
    // ==========================================================================

    #[test]
    fn test_point_on_segment_2d_middle() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let p = Point2::new(1.0, 0.0);
        assert!(point_on_segment_2d(&p, &a, &b));
    }

    #[test]
    fn test_point_on_segment_2d_endpoint() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        assert!(point_on_segment_2d(&a, &a, &b));
        assert!(point_on_segment_2d(&b, &a, &b));
    }

    #[test]
    fn test_point_on_segment_2d_off_segment() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let p = Point2::new(1.0, 0.1);
        assert!(!point_on_segment_2d(&p, &a, &b));
    }

    #[test]
    fn test_point_on_segment_2d_collinear_but_outside() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(2.0, 0.0);
        let p = Point2::new(3.0, 0.0);
        assert!(!point_on_segment_2d(&p, &a, &b));
    }

    #[test]
    fn test_point_on_segment_2d_degenerate_segment() {
        // a == b: only p == a should count as "on segment"
        let a = Point2::new(1.0, 1.0);
        assert!(point_on_segment_2d(&a, &a, &a));
        let p = Point2::new(2.0, 2.0);
        assert!(!point_on_segment_2d(&p, &a, &a));
    }

    #[test]
    fn test_point_on_plane_on() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let p = Point3::new(0.5, 0.5, 0.0);
        assert!(point_on_plane(&p, &a, &b, &c));
    }

    #[test]
    fn test_point_on_plane_off() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let p = Point3::new(0.5, 0.5, 0.1);
        assert!(!point_on_plane(&p, &a, &b, &c));
    }

    #[test]
    fn test_are_coplanar() {
        let a = Point3::new(0.0, 0.0, 0.0);
        let b = Point3::new(1.0, 0.0, 0.0);
        let c = Point3::new(0.0, 1.0, 0.0);
        let d = Point3::new(1.0, 1.0, 0.0);
        assert!(are_coplanar(&a, &b, &c, &d));

        let e = Point3::new(1.0, 1.0, 1.0);
        assert!(!are_coplanar(&a, &b, &c, &e));
    }

    #[test]
    fn test_are_collinear_2d() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 1.0);
        let c = Point2::new(2.0, 2.0);
        assert!(are_collinear_2d(&a, &b, &c));

        let d = Point2::new(2.0, 2.1);
        assert!(!are_collinear_2d(&a, &b, &d));
    }

    #[test]
    fn test_point_side_of_line() {
        let a = Point2::new(0.0, 0.0);
        let b = Point2::new(1.0, 0.0);
        assert_eq!(
            point_side_of_line(&Point2::new(0.5, 1.0), &a, &b),
            Some(Sign::Positive)
        );
        assert_eq!(
            point_side_of_line(&Point2::new(0.5, -1.0), &a, &b),
            Some(Sign::Negative)
        );
        // Collinear points (including endpoints) return None
        assert_eq!(point_side_of_line(&Point2::new(0.5, 0.0), &a, &b), None);
        assert_eq!(point_side_of_line(&a, &a, &b), None);
    }
}
