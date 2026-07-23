//! Constraint types for the 2D sketch solver.
//!
//! Constraints define relationships between sketch entities that the solver
//! must satisfy. They are divided into geometric constraints (dimensionless)
//! and dimensional constraints (with explicit values).

use crate::entity::EntityId;

/// Reference to a point within an entity.
///
/// Used to specify which point of a multi-point entity (like a line) to use
/// in a constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityRef {
    /// A point entity directly.
    Point(EntityId),
    /// The start point of a line entity.
    LineStart(EntityId),
    /// The end point of a line entity.
    LineEnd(EntityId),
    /// The center point of an arc or circle entity.
    Center(EntityId),
    /// The start point of an arc entity.
    ArcStart(EntityId),
    /// The end point of an arc entity.
    ArcEnd(EntityId),
}

/// A constraint on sketch entities.
///
/// Constraints are expressed as error functions that should equal zero when
/// satisfied. The solver minimizes the sum of squared errors.
#[derive(Debug, Clone)]
pub enum Constraint {
    // =========================================================================
    // Geometric constraints (no explicit dimension)
    // =========================================================================
    /// Two points are at the same location.
    ///
    /// Error: `[p1.x - p2.x, p1.y - p2.y]`
    Coincident {
        /// First point reference.
        point_a: EntityRef,
        /// Second point reference.
        point_b: EntityRef,
    },

    /// A point lies on a line (extended infinitely).
    ///
    /// Error: signed distance from point to line
    PointOnLine {
        /// Point to constrain.
        point: EntityRef,
        /// Line entity the point must lie on.
        line: EntityId,
    },

    /// Two lines are parallel.
    ///
    /// Error: `cross(d1, d2) / (|d1| |d2|)` where d = end - start
    Parallel {
        /// First line entity.
        line_a: EntityId,
        /// Second line entity.
        line_b: EntityId,
    },

    /// Two lines are perpendicular.
    ///
    /// Error: `dot(d1, d2) / (|d1| |d2|)` where d = end - start
    Perpendicular {
        /// First line entity.
        line_a: EntityId,
        /// Second line entity.
        line_b: EntityId,
    },

    /// A line is horizontal (parallel to X axis).
    ///
    /// Error: `end.y - start.y`
    Horizontal {
        /// Line entity to constrain.
        line: EntityId,
    },

    /// A line is vertical (parallel to Y axis).
    ///
    /// Error: `end.x - start.x`
    Vertical {
        /// Line entity to constrain.
        line: EntityId,
    },

    /// A line is tangent to an arc or circle at a point.
    ///
    /// Error: dot product of line direction and radius vector at the tangent point
    Tangent {
        /// Line entity.
        line: EntityId,
        /// Arc or circle entity.
        curve: EntityId,
        /// Point where tangency occurs.
        at_point: EntityRef,
    },

    /// Two lines have equal length.
    ///
    /// Error: `|line_a| - |line_b|`
    EqualLength {
        /// First line entity.
        line_a: EntityId,
        /// Second line entity.
        line_b: EntityId,
    },

    /// Two arcs or circles have equal radius.
    ///
    /// Error: `r1 - r2`
    EqualRadius {
        /// First circle/arc entity.
        circle_a: EntityId,
        /// Second circle/arc entity.
        circle_b: EntityId,
    },

    /// Two arcs or circles share the same center.
    ///
    /// Error: `[c1.x - c2.x, c1.y - c2.y]`
    Concentric {
        /// First circle/arc entity.
        circle_a: EntityId,
        /// Second circle/arc entity.
        circle_b: EntityId,
    },

    /// A point is fixed at a specific location.
    ///
    /// Error: `[p.x - x, p.y - y]`
    Fixed {
        /// Point to fix.
        point: EntityRef,
        /// Target X coordinate.
        x: f64,
        /// Target Y coordinate.
        y: f64,
    },

    /// Two points hold a fixed offset: `point_b = point_a + (dx, dy)`.
    ///
    /// A rigid link between two free points — e.g. a pad center riding a
    /// footprint origin. Error: `[b.x - a.x - dx, b.y - a.y - dy]`
    OffsetCoincident {
        /// Base point reference.
        point_a: EntityRef,
        /// Offset point reference.
        point_b: EntityRef,
        /// Offset along X.
        dx: f64,
        /// Offset along Y.
        dy: f64,
    },

    /// A point lies on an arc or circle.
    ///
    /// Error: `|p - center| - radius`
    PointOnCircle {
        /// Point to constrain.
        point: EntityRef,
        /// Circle/arc entity.
        circle: EntityId,
    },

    /// A line passes through the center of a circle.
    ///
    /// Error: signed distance from center to line
    LineThroughCenter {
        /// Line entity.
        line: EntityId,
        /// Circle entity.
        circle: EntityId,
    },

    /// A point is at the midpoint of a line.
    ///
    /// Error: `[p.x - (start.x + end.x)/2, p.y - (start.y + end.y)/2]`
    Midpoint {
        /// Point to constrain.
        point: EntityRef,
        /// Line entity.
        line: EntityId,
    },

    /// Two points are symmetric about a line.
    ///
    /// Error: combination of perpendicular distance and equal distance from line
    Symmetric {
        /// First point.
        point_a: EntityRef,
        /// Second point.
        point_b: EntityRef,
        /// Axis of symmetry (line entity).
        axis: EntityId,
    },

    // =========================================================================
    // Dimensional constraints (explicit values)
    // =========================================================================
    /// Distance between two points equals a value.
    ///
    /// Error: `|p1 - p2| - distance`
    Distance {
        /// First point reference.
        point_a: EntityRef,
        /// Second point reference.
        point_b: EntityRef,
        /// Target distance.
        distance: f64,
    },

    /// Perpendicular distance from a point to a line.
    ///
    /// Error: `|signed_distance| - distance`
    PointLineDistance {
        /// Point reference.
        point: EntityRef,
        /// Line entity.
        line: EntityId,
        /// Target distance.
        distance: f64,
    },

    /// Angle between two lines.
    ///
    /// Error: `actual_angle - angle_rad`
    Angle {
        /// First line entity.
        line_a: EntityId,
        /// Second line entity.
        line_b: EntityId,
        /// Target angle in radians.
        angle_rad: f64,
    },

    /// Radius of a circle equals a value.
    ///
    /// Error: `r_actual - radius`
    Radius {
        /// Circle entity.
        circle: EntityId,
        /// Target radius.
        radius: f64,
    },

    /// Length of a line equals a value.
    ///
    /// Error: `|line| - length`
    Length {
        /// Line entity.
        line: EntityId,
        /// Target length.
        length: f64,
    },

    /// X coordinate of a point equals a value.
    ///
    /// Error: `p.x - x`
    HorizontalDistance {
        /// Point reference.
        point: EntityRef,
        /// Target X coordinate.
        x: f64,
    },

    /// Y coordinate of a point equals a value.
    ///
    /// Error: `p.y - y`
    VerticalDistance {
        /// Point reference.
        point: EntityRef,
        /// Target Y coordinate.
        y: f64,
    },

    /// Diameter of a circle equals a value.
    ///
    /// Error: `2 * r_actual - diameter`
    Diameter {
        /// Circle entity.
        circle: EntityId,
        /// Target diameter.
        diameter: f64,
    },
}

impl Constraint {
    /// Returns the number of scalar error values this constraint contributes.
    ///
    /// Most constraints contribute 1 error, but some (like Coincident, Fixed)
    /// contribute 2 (one for X, one for Y).
    pub fn num_residuals(&self) -> usize {
        match self {
            Constraint::Coincident { .. } => 2,
            Constraint::OffsetCoincident { .. } => 2,
            Constraint::Fixed { .. } => 2,
            Constraint::Concentric { .. } => 2,
            Constraint::Midpoint { .. } => 2,
            Constraint::Symmetric { .. } => 2,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_num_residuals() {
        let coincident = Constraint::Coincident {
            point_a: EntityRef::Point(EntityId::default()),
            point_b: EntityRef::Point(EntityId::default()),
        };
        assert_eq!(coincident.num_residuals(), 2);

        let horizontal = Constraint::Horizontal {
            line: EntityId::default(),
        };
        assert_eq!(horizontal.num_residuals(), 1);

        let fixed = Constraint::Fixed {
            point: EntityRef::Point(EntityId::default()),
            x: 0.0,
            y: 0.0,
        };
        assert_eq!(fixed.num_residuals(), 2);
    }
}
