//! CAM operation definitions.

use crate::{CamError, CamSettings, Tool, Toolpath};
use serde::{Deserialize, Serialize};

mod contour;
mod face;
mod pocket;
mod roughing3d;

pub use contour::{Contour2D, Tab};
pub use face::Face;
pub use pocket::Pocket2D;
pub use roughing3d::Roughing3D;

/// A CAM operation that can generate a toolpath.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CamOperation {
    /// Surface facing operation.
    Face(Face),
    /// 2D pocket clearing operation.
    Pocket2D(Pocket2D),
    /// 2D contour/profile operation.
    Contour2D(Contour2D),
    /// 3D roughing operation.
    Roughing3D(Roughing3D),
}

impl CamOperation {
    /// Generate a toolpath for this operation.
    ///
    /// Note: For Roughing3D, use `generate_with_height_field` instead.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        match self {
            CamOperation::Face(op) => op.generate(tool, settings),
            CamOperation::Pocket2D(op) => op.generate(tool, settings),
            CamOperation::Contour2D(op) => op.generate(tool, settings),
            CamOperation::Roughing3D(_) => Err(CamError::EmptyContour), // Need height field
        }
    }

    /// Generate a toolpath for Roughing3D operation with a height field.
    pub fn generate_with_height_field(
        &self,
        height_field: &crate::dropcutter::HeightField,
        tool: &Tool,
        settings: &CamSettings,
    ) -> Result<Toolpath, CamError> {
        match self {
            CamOperation::Roughing3D(op) => op.generate(height_field, tool, settings),
            _ => self.generate(tool, settings),
        }
    }

    /// Get a descriptive name for this operation type.
    pub fn name(&self) -> &'static str {
        match self {
            CamOperation::Face(_) => "Face",
            CamOperation::Pocket2D(_) => "Pocket 2D",
            CamOperation::Contour2D(_) => "Contour 2D",
            CamOperation::Roughing3D(_) => "Roughing 3D",
        }
    }

    /// Check if this operation requires a height field.
    pub fn requires_height_field(&self) -> bool {
        matches!(self, CamOperation::Roughing3D(_))
    }
}

/// A 2D point for contour definitions.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point2D {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl Point2D {
    /// Create a new 2D point.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Distance to another point.
    pub fn distance_to(&self, other: &Point2D) -> f64 {
        let dx = other.x - self.x;
        let dy = other.y - self.y;
        (dx * dx + dy * dy).sqrt()
    }
}

impl From<(f64, f64)> for Point2D {
    fn from((x, y): (f64, f64)) -> Self {
        Self::new(x, y)
    }
}

/// A 2D contour segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContourSegment {
    /// Line segment.
    Line {
        /// End point.
        to: Point2D,
    },
    /// Arc segment.
    Arc {
        /// End point.
        to: Point2D,
        /// Arc center.
        center: Point2D,
        /// Counter-clockwise direction.
        ccw: bool,
    },
}

/// A closed 2D contour made of line and arc segments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contour {
    /// Starting point.
    pub start: Point2D,
    /// Segments forming the contour.
    pub segments: Vec<ContourSegment>,
}

impl Contour {
    /// Create a new contour starting at the given point.
    pub fn new(start: Point2D) -> Self {
        Self {
            start,
            segments: Vec::new(),
        }
    }

    /// Add a line segment to the contour.
    pub fn line_to(&mut self, to: Point2D) {
        self.segments.push(ContourSegment::Line { to });
    }

    /// Add an arc segment to the contour.
    pub fn arc_to(&mut self, to: Point2D, center: Point2D, ccw: bool) {
        self.segments.push(ContourSegment::Arc { to, center, ccw });
    }

    /// Create a rectangular contour.
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Self {
        let mut contour = Self::new(Point2D::new(x, y));
        contour.line_to(Point2D::new(x + width, y));
        contour.line_to(Point2D::new(x + width, y + height));
        contour.line_to(Point2D::new(x, y + height));
        contour.line_to(Point2D::new(x, y));
        contour
    }

    /// Create a circular contour.
    pub fn circle(cx: f64, cy: f64, radius: f64) -> Self {
        let mut contour = Self::new(Point2D::new(cx + radius, cy));
        // Two semicircles
        contour.arc_to(
            Point2D::new(cx - radius, cy),
            Point2D::new(cx, cy),
            true, // CCW
        );
        contour.arc_to(
            Point2D::new(cx + radius, cy),
            Point2D::new(cx, cy),
            true, // CCW
        );
        contour
    }

    /// Check if the contour is closed (within tolerance).
    pub fn is_closed(&self, tolerance: f64) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        let end = self.end_point();
        self.start.distance_to(&end) < tolerance
    }

    /// Get the end point of the contour.
    pub fn end_point(&self) -> Point2D {
        match self.segments.last() {
            Some(ContourSegment::Line { to }) => *to,
            Some(ContourSegment::Arc { to, .. }) => *to,
            None => self.start,
        }
    }

    /// Calculate the approximate perimeter length.
    pub fn perimeter(&self) -> f64 {
        let mut length = 0.0;
        let mut current = self.start;

        for seg in &self.segments {
            match seg {
                ContourSegment::Line { to } => {
                    length += current.distance_to(to);
                    current = *to;
                }
                ContourSegment::Arc { to, center, .. } => {
                    // Approximate arc length
                    let r = center.distance_to(&current);
                    let dx1 = current.x - center.x;
                    let dy1 = current.y - center.y;
                    let dx2 = to.x - center.x;
                    let dy2 = to.y - center.y;
                    let angle1 = dy1.atan2(dx1);
                    let angle2 = dy2.atan2(dx2);
                    let mut delta = angle2 - angle1;
                    if delta < 0.0 {
                        delta += 2.0 * std::f64::consts::PI;
                    }
                    length += r * delta;
                    current = *to;
                }
            }
        }

        length
    }

    /// Check if the contour is roughly circular (made of arc segments only).
    pub fn is_circular(&self) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        self.segments
            .iter()
            .all(|s| matches!(s, ContourSegment::Arc { .. }))
    }

    /// Convert to geo crate LineString for offset operations.
    pub fn to_geo_polygon(&self) -> geo::Polygon<f64> {
        let mut coords = vec![geo::Coord {
            x: self.start.x,
            y: self.start.y,
        }];

        for seg in &self.segments {
            match seg {
                ContourSegment::Line { to } => {
                    coords.push(geo::Coord { x: to.x, y: to.y });
                }
                ContourSegment::Arc { to, center, ccw } => {
                    // Linearize arc into segments
                    let current = coords.last().unwrap();
                    let r =
                        ((center.x - current.x).powi(2) + (center.y - current.y).powi(2)).sqrt();
                    let start_angle = (current.y - center.y).atan2(current.x - center.x);
                    let end_angle = (to.y - center.y).atan2(to.x - center.x);

                    let mut delta = if *ccw {
                        end_angle - start_angle
                    } else {
                        start_angle - end_angle
                    };
                    if delta < 0.0 {
                        delta += 2.0 * std::f64::consts::PI;
                    }

                    // Use approximately 5 degree segments
                    let segments = ((delta.abs() / 0.087).ceil() as usize).max(1);
                    let step = delta / segments as f64;

                    for i in 1..=segments {
                        let angle = if *ccw {
                            start_angle + step * i as f64
                        } else {
                            start_angle - step * i as f64
                        };
                        let px = center.x + r * angle.cos();
                        let py = center.y + r * angle.sin();
                        coords.push(geo::Coord { x: px, y: py });
                    }
                }
            }
        }

        geo::Polygon::new(geo::LineString::from(coords), vec![])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point2d() {
        let p1 = Point2D::new(0.0, 0.0);
        let p2 = Point2D::new(3.0, 4.0);
        assert!((p1.distance_to(&p2) - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_contour_rectangle() {
        let rect = Contour::rectangle(0.0, 0.0, 10.0, 5.0);
        assert_eq!(rect.segments.len(), 4);
        assert!(rect.is_closed(1e-6));
        assert!((rect.perimeter() - 30.0).abs() < 1e-6);
    }

    #[test]
    fn test_contour_circle() {
        let circle = Contour::circle(0.0, 0.0, 10.0);
        assert_eq!(circle.segments.len(), 2);
        assert!(circle.is_closed(1e-6));
        // Perimeter should be approximately 2*PI*r = 62.83
        let expected = 2.0 * std::f64::consts::PI * 10.0;
        assert!((circle.perimeter() - expected).abs() < 0.1);
    }
}
