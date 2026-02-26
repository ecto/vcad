#![warn(missing_docs)]

//! 2D sketch constraint solver for the vcad kernel.
//!
//! This crate provides a constraint-based sketch system where geometric and
//! dimensional constraints are enforced automatically using Levenberg-Marquardt
//! optimization.
//!
//! # Overview
//!
//! The solver works by:
//! 1. Representing sketch geometry (points, lines, arcs, circles) as parameters
//! 2. Defining constraints as error functions that should equal zero
//! 3. Using Levenberg-Marquardt to minimize the sum of squared errors
//!
//! # Example
//!
//! ```
//! use vcad_kernel_constraints::{Sketch2D, EntityRef, Constraint};
//!
//! // Create a sketch
//! let mut sketch = Sketch2D::new();
//!
//! // Add points for a rectangle
//! let p0 = sketch.add_point(0.0, 0.0);
//! let p1 = sketch.add_point(12.0, 1.0);  // Intentionally offset
//! let p2 = sketch.add_point(11.0, 6.0);
//! let p3 = sketch.add_point(1.0, 5.0);
//!
//! // Create lines
//! let l0 = sketch.add_line(p0, p1);
//! let l1 = sketch.add_line(p1, p2);
//! let l2 = sketch.add_line(p2, p3);
//! let l3 = sketch.add_line(p3, p0);
//!
//! // Add constraints
//! sketch.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
//! sketch.constrain_horizontal(l0);
//! sketch.constrain_horizontal(l2);
//! sketch.constrain_vertical(l1);
//! sketch.constrain_vertical(l3);
//! sketch.constrain_length(l0, 10.0);
//! sketch.constrain_length(l1, 5.0);
//!
//! // Solve
//! let result = sketch.solve_default();
//! assert!(result.converged);
//!
//! // The points are now at the correct positions for a 10x5 rectangle
//! let (x1, y1) = sketch.get_point(p1).unwrap();
//! assert!((x1 - 10.0).abs() < 1e-6);
//! assert!((y1 - 0.0).abs() < 1e-6);
//! ```
//!
//! # Degrees of Freedom
//!
//! The solver tracks degrees of freedom (DOF):
//! - Each point adds 2 DOF (x, y coordinates)
//! - Each circle adds 1 additional DOF (radius)
//! - Each constraint removes DOF equal to its number of residual equations
//!
//! A fully constrained sketch has DOF = 0.
//!
//! # Exporting to SketchProfile
//!
//! After solving, a sketch can be exported to a `SketchProfile` for use with
//! extrude and revolve operations:
//!
//! ```
//! use vcad_kernel_constraints::{Sketch2D, EntityRef};
//!
//! let mut sketch = Sketch2D::new();
//! // ... add entities and constraints ...
//! # let p0 = sketch.add_point(0.0, 0.0);
//! # let p1 = sketch.add_point(10.0, 0.0);
//! # let p2 = sketch.add_point(10.0, 5.0);
//! # let p3 = sketch.add_point(0.0, 5.0);
//! # sketch.add_line(p0, p1);
//! # sketch.add_line(p1, p2);
//! # sketch.add_line(p2, p3);
//! # sketch.add_line(p3, p0);
//!
//! sketch.solve_default();
//!
//! // Export to SketchProfile
//! let profile = sketch.to_profile().unwrap();
//! ```

mod constraint;
mod entity;
mod export;
pub mod gpu;
mod jacobian;
mod residual;
mod sketch;
mod solver;
pub mod symbolic;

pub use constraint::{Constraint, EntityRef};
pub use entity::{EntityId, SketchArc, SketchCircle, SketchEntity, SketchLine, SketchPoint};
pub use export::ExportError;
pub use sketch::Sketch2D;
pub use solver::{SolveResult, SolveStatus, SolverConfig};

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_kernel_math::Vec3;

    #[test]
    fn test_constrained_triangle() {
        let mut sketch = Sketch2D::new();

        // Create an equilateral triangle
        let p0 = sketch.add_point(0.0, 0.0);
        let p1 = sketch.add_point(10.0, 0.0);
        let p2 = sketch.add_point(5.0, 8.0);

        let l0 = sketch.add_line(p0, p1);
        let l1 = sketch.add_line(p1, p2);
        let l2 = sketch.add_line(p2, p0);

        // Fix base
        sketch.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
        sketch.constrain_horizontal(l0);
        sketch.constrain_length(l0, 10.0);

        // Equal sides
        sketch.constrain_equal_length(l0, l1);
        sketch.constrain_equal_length(l1, l2);

        let result = sketch.solve_default();
        assert!(result.converged, "Triangle should converge");

        // Check all sides are equal
        let len0 = sketch.get_line_length(l0).unwrap();
        let len1 = sketch.get_line_length(l1).unwrap();
        let len2 = sketch.get_line_length(l2).unwrap();

        assert!((len0 - 10.0).abs() < 1e-5);
        assert!((len1 - len0).abs() < 1e-5);
        assert!((len2 - len0).abs() < 1e-5);
    }

    #[test]
    fn test_perpendicular_lines() {
        let mut sketch = Sketch2D::new();

        let p0 = sketch.add_point(0.0, 0.0);
        let p1 = sketch.add_point(10.0, 2.0);
        let p2 = sketch.add_point(12.0, 12.0);

        let l0 = sketch.add_line(p0, p1);
        let l1 = sketch.add_line(p1, p2);

        sketch.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
        sketch.constrain_horizontal(l0);
        sketch.constrain_perpendicular(l0, l1);
        sketch.constrain_length(l0, 10.0);
        sketch.constrain_length(l1, 8.0);

        let result = sketch.solve_default();
        assert!(result.converged);

        // L0 should be horizontal, L1 should be vertical
        let ((x0, y0), (x1, y1)) = sketch.get_line_endpoints(l0).unwrap();
        let ((_, _), (x2, y2)) = sketch.get_line_endpoints(l1).unwrap();

        assert!((y0 - y1).abs() < 1e-6, "L0 should be horizontal");
        assert!((x1 - x2).abs() < 1e-6, "L1 should be vertical");
        assert!((y2 - y1 - 8.0).abs() < 1e-5, "L1 length should be 8");
        assert!((x1 - x0 - 10.0).abs() < 1e-5, "L0 length should be 10");
    }

    #[test]
    fn test_circle_with_constraints() {
        let mut sketch = Sketch2D::new();

        let (circle, center) = sketch.add_circle_by_coords(5.0, 5.0, 3.0);

        // Fix center and set radius
        sketch.constrain_fixed(EntityRef::Point(center), 10.0, 10.0);
        sketch.constrain_radius(circle, 5.0);

        let result = sketch.solve_default();
        assert!(result.converged);

        let (cx, cy) = sketch.get_point(center).unwrap();
        let r = sketch.get_radius(circle).unwrap();

        assert!((cx - 10.0).abs() < 1e-6);
        assert!((cy - 10.0).abs() < 1e-6);
        assert!((r - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_export_and_extrude() {
        use vcad_kernel_sketch::extrude;

        let mut sketch = Sketch2D::new();

        // Create a 10x5 rectangle
        let p0 = sketch.add_point(0.0, 0.0);
        let p1 = sketch.add_point(10.0, 0.0);
        let p2 = sketch.add_point(10.0, 5.0);
        let p3 = sketch.add_point(0.0, 5.0);

        sketch.add_line(p0, p1);
        sketch.add_line(p1, p2);
        sketch.add_line(p2, p3);
        sketch.add_line(p3, p0);

        // Export and extrude
        let profile = sketch.to_profile().unwrap();
        let solid = extrude(&profile, Vec3::new(0.0, 0.0, 20.0)).unwrap();

        // Should have 6 faces (box)
        assert_eq!(solid.topology.faces.len(), 6);
    }

    #[test]
    fn test_dof_calculation() {
        let mut sketch = Sketch2D::new();

        // 4 points = 8 DOF
        let p0 = sketch.add_point(0.0, 0.0);
        let p1 = sketch.add_point(10.0, 0.0);
        let p2 = sketch.add_point(10.0, 5.0);
        let p3 = sketch.add_point(0.0, 5.0);

        assert_eq!(sketch.degrees_of_freedom(), 8);

        let l0 = sketch.add_line(p0, p1);
        let l1 = sketch.add_line(p1, p2);
        let l2 = sketch.add_line(p2, p3);
        let l3 = sketch.add_line(p3, p0);

        // Lines don't add parameters
        assert_eq!(sketch.degrees_of_freedom(), 8);

        // Fix origin: -2 DOF
        sketch.constrain_fixed(EntityRef::Point(p0), 0.0, 0.0);
        assert_eq!(sketch.degrees_of_freedom(), 6);

        // Horizontal constraints: -2 DOF
        sketch.constrain_horizontal(l0);
        sketch.constrain_horizontal(l2);
        assert_eq!(sketch.degrees_of_freedom(), 4);

        // Vertical constraints: -2 DOF
        sketch.constrain_vertical(l1);
        sketch.constrain_vertical(l3);
        assert_eq!(sketch.degrees_of_freedom(), 2);

        // Length constraints: -2 DOF
        sketch.constrain_length(l0, 10.0);
        sketch.constrain_length(l1, 5.0);
        assert_eq!(sketch.degrees_of_freedom(), 0);

        assert!(sketch.is_fully_constrained());
    }
}
