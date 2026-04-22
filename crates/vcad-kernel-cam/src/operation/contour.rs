//! 2D contour/profile machining operation.

use crate::operation::{Contour, ContourSegment, Point2D};
use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
#[cfg(not(target_arch = "wasm32"))]
use geo_clipper::Clipper;
use serde::{Deserialize, Serialize};

/// A holding tab to prevent part from moving during cutout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tab {
    /// Position along the contour as a fraction (0.0 to 1.0).
    pub position: f64,
    /// Width of the tab in mm.
    pub width: f64,
    /// Height of the tab (how much material to leave).
    pub height: f64,
}

impl Tab {
    /// Create a new tab.
    pub fn new(position: f64, width: f64, height: f64) -> Self {
        Self {
            position,
            width,
            height,
        }
    }
}

/// 2D contour/profile machining operation.
///
/// Machines along the outside or inside of a contour with optional tabs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contour2D {
    /// The contour to machine.
    pub contour: Contour,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
    /// Offset from contour (positive = outside, negative = inside).
    /// This is in addition to the tool radius compensation.
    pub offset: f64,
    /// Holding tabs to prevent part movement.
    pub tabs: Vec<Tab>,
    /// Stock to leave for finishing pass.
    pub stock_to_leave: f64,
}

impl Contour2D {
    /// Create a new contour operation.
    pub fn new(contour: Contour, depth: f64) -> Self {
        Self {
            contour,
            depth,
            offset: 0.0,
            tabs: Vec::new(),
            stock_to_leave: 0.0,
        }
    }

    /// Set the offset from contour.
    pub fn with_offset(mut self, offset: f64) -> Self {
        self.offset = offset;
        self
    }

    /// Add a tab.
    pub fn with_tab(mut self, tab: Tab) -> Self {
        self.tabs.push(tab);
        self
    }

    /// Add multiple evenly-spaced tabs.
    pub fn with_tabs(mut self, count: usize, width: f64, height: f64) -> Self {
        for i in 0..count {
            let position = i as f64 / count as f64;
            self.tabs.push(Tab::new(position, width, height));
        }
        self
    }

    /// Set stock to leave.
    pub fn with_stock_to_leave(mut self, stock: f64) -> Self {
        self.stock_to_leave = stock;
        self
    }

    /// Create an outside contour for cutting out a part.
    pub fn outside(contour: Contour, depth: f64) -> Self {
        Self::new(contour, depth)
    }

    /// Create an inside contour for cutting a hole.
    pub fn inside(contour: Contour, depth: f64) -> Self {
        Self::new(contour, depth)
    }

    /// Generate the toolpath for this contour operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        // Validate inputs
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepdown <= 0.0 {
            return Err(CamError::InvalidStepdown(settings.stepdown));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }
        if !self.contour.is_closed(0.01) {
            let gap = self.contour.start.distance_to(&self.contour.end_point());
            return Err(CamError::NotClosed(gap));
        }

        // Validate tab positions
        for tab in &self.tabs {
            if tab.position < 0.0 || tab.position > 1.0 {
                return Err(CamError::InvalidTabPosition(tab.position));
            }
        }

        let mut toolpath = Toolpath::new();

        toolpath.push(ToolpathSegment::comment(format!(
            "Contour 2D: depth={:.2}mm, offset={:.2}mm, {} tabs",
            self.depth,
            self.offset,
            self.tabs.len()
        )));

        // Calculate offset path
        let tool_radius = tool.radius();
        let total_offset = tool_radius + self.offset + self.stock_to_leave;
        let offset_contour = self.offset_contour(total_offset)?;

        // Calculate Z levels
        let num_z_passes = (self.depth / settings.stepdown).ceil() as usize;
        let z_step = self.depth / num_z_passes as f64;

        // Calculate tab Z height (from bottom)
        let tab_z = if !self.tabs.is_empty() {
            Some(-self.depth + self.tabs[0].height)
        } else {
            None
        };

        for z_pass in 0..num_z_passes {
            let z = -((z_pass + 1) as f64) * z_step;
            let is_final_pass = z_pass == num_z_passes - 1;

            toolpath.push(ToolpathSegment::comment(format!("Z level: {:.3}", z)));

            // Get contour points
            let points = &offset_contour;
            if points.is_empty() {
                continue;
            }

            // Move to start
            let start = &points[0];
            toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));
            toolpath.push(ToolpathSegment::linear(
                start.x,
                start.y,
                z,
                settings.plunge_rate,
            ));

            // Follow contour with tab handling on final pass
            if is_final_pass && !self.tabs.is_empty() {
                if let Some(tab_height) = tab_z {
                    self.generate_with_tabs(
                        &mut toolpath,
                        points,
                        z,
                        tab_height,
                        settings.feed_rate,
                    );
                }
            } else {
                // Regular contour following
                for point in points.iter().skip(1) {
                    toolpath.push(ToolpathSegment::linear(
                        point.x,
                        point.y,
                        z,
                        settings.feed_rate,
                    ));
                }
                // Close the contour
                toolpath.push(ToolpathSegment::linear(
                    start.x,
                    start.y,
                    z,
                    settings.feed_rate,
                ));
            }

            // Retract between passes
            if !is_final_pass {
                toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));
            }
        }

        // Final retract
        toolpath.push(ToolpathSegment::rapid(
            self.contour.start.x,
            self.contour.start.y,
            settings.safe_z,
        ));

        Ok(toolpath)
    }

    /// Offset the contour by the given amount (native version with geo-clipper).
    #[cfg(not(target_arch = "wasm32"))]
    fn offset_contour(&self, offset: f64) -> Result<Vec<Point2D>, CamError> {
        if offset.abs() < 0.001 {
            // No offset needed, return original points
            return Ok(self.contour_to_points(&self.contour));
        }

        // Use geo-clipper for offset
        let polygon = self.contour.to_geo_polygon();
        let scale = 1000.0;

        let result = polygon.offset(
            offset * scale,
            geo_clipper::JoinType::Round(10.0),
            geo_clipper::EndType::ClosedPolygon,
            scale,
        );

        if result.0.is_empty() {
            return Err(CamError::EmptyContour);
        }

        // Extract points from first polygon
        if let Some(poly) = result.0.first() {
            let exterior = poly.exterior();
            Ok(exterior.0.iter().map(|c| Point2D::new(c.x, c.y)).collect())
        } else {
            Err(CamError::EmptyContour)
        }
    }

    /// Offset the contour by the given amount (WASM version with simple offset).
    ///
    /// This is a simplified implementation for rectangular and circular contours.
    #[cfg(target_arch = "wasm32")]
    fn offset_contour(&self, offset: f64) -> Result<Vec<Point2D>, CamError> {
        use geo::BoundingRect;

        if offset.abs() < 0.001 {
            return Ok(self.contour_to_points(&self.contour));
        }

        let polygon = self.contour.to_geo_polygon();
        let Some(bbox) = polygon.bounding_rect() else {
            return Err(CamError::EmptyContour);
        };

        let width = bbox.width();
        let height = bbox.height();
        let cx = bbox.min().x + width / 2.0;
        let cy = bbox.min().y + height / 2.0;

        // For simple offset, expand/contract the bounding rectangle
        let is_circular = self.contour.is_circular();

        if is_circular {
            // Circular offset
            let radius = width.min(height) / 2.0 + offset;
            if radius <= 0.0 {
                return Err(CamError::EmptyContour);
            }

            let segments = 36;
            let points: Vec<Point2D> = (0..=segments)
                .map(|i| {
                    let angle = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                    Point2D::new(cx + radius * angle.cos(), cy + radius * angle.sin())
                })
                .collect();
            Ok(points)
        } else {
            // Rectangular offset
            let half_w = width / 2.0 + offset;
            let half_h = height / 2.0 + offset;

            if half_w <= 0.0 || half_h <= 0.0 {
                return Err(CamError::EmptyContour);
            }

            Ok(vec![
                Point2D::new(cx - half_w, cy - half_h),
                Point2D::new(cx + half_w, cy - half_h),
                Point2D::new(cx + half_w, cy + half_h),
                Point2D::new(cx - half_w, cy + half_h),
                Point2D::new(cx - half_w, cy - half_h),
            ])
        }
    }

    /// Convert contour to a list of points.
    fn contour_to_points(&self, contour: &Contour) -> Vec<Point2D> {
        let mut points = vec![contour.start];

        for seg in &contour.segments {
            match seg {
                ContourSegment::Line { to } => {
                    points.push(*to);
                }
                ContourSegment::Arc { to, center, ccw } => {
                    // Linearize arc
                    let current = points.last().unwrap();
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

                    let segments = ((delta.abs() / 0.087).ceil() as usize).max(1);
                    let step = delta / segments as f64;

                    for i in 1..=segments {
                        let angle = if *ccw {
                            start_angle + step * i as f64
                        } else {
                            start_angle - step * i as f64
                        };
                        points.push(Point2D::new(
                            center.x + r * angle.cos(),
                            center.y + r * angle.sin(),
                        ));
                    }
                }
            }
        }

        points
    }

    /// Generate toolpath with tabs on final pass.
    fn generate_with_tabs(
        &self,
        toolpath: &mut Toolpath,
        points: &[Point2D],
        cut_z: f64,
        tab_z: f64,
        feed: f64,
    ) {
        if points.is_empty() {
            return;
        }

        // Calculate cumulative distances
        let mut cumulative_dist = vec![0.0];
        let mut total_dist = 0.0;

        for i in 1..points.len() {
            let dist = points[i - 1].distance_to(&points[i]);
            total_dist += dist;
            cumulative_dist.push(total_dist);
        }

        // Add distance to close the loop
        let close_dist = points.last().unwrap().distance_to(&points[0]);
        total_dist += close_dist;

        // Sort tabs by position
        let mut sorted_tabs: Vec<_> = self.tabs.iter().collect();
        sorted_tabs.sort_by(|a, b| a.position.total_cmp(&b.position));

        // Generate path with tabs
        let mut in_tab = false;

        for point_idx in 1..=points.len() {
            let point = if point_idx == points.len() {
                &points[0]
            } else {
                &points[point_idx]
            };

            let point_dist = if point_idx == points.len() {
                total_dist
            } else {
                cumulative_dist[point_idx]
            };
            let point_fraction = point_dist / total_dist;

            // Check if we're in a tab region
            let mut currently_in_tab = false;
            for tab in &sorted_tabs {
                let tab_half_width = (tab.width / total_dist) / 2.0;
                let tab_start = (tab.position - tab_half_width).max(0.0);
                let tab_end = (tab.position + tab_half_width).min(1.0);

                if point_fraction >= tab_start && point_fraction <= tab_end {
                    currently_in_tab = true;
                    break;
                }
            }

            // Handle transition in/out of tab
            if currently_in_tab && !in_tab {
                // Entering tab: raise Z
                toolpath.push(ToolpathSegment::linear(point.x, point.y, tab_z, feed));
                in_tab = true;
            } else if !currently_in_tab && in_tab {
                // Leaving tab: lower Z
                toolpath.push(ToolpathSegment::linear(point.x, point.y, cut_z, feed));
                in_tab = false;
            } else {
                // Normal move at current Z
                let z = if in_tab { tab_z } else { cut_z };
                toolpath.push(ToolpathSegment::linear(point.x, point.y, z, feed));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contour2d_basic() {
        let contour = Contour::rectangle(0.0, 0.0, 30.0, 20.0);
        let op = Contour2D::new(contour, 5.0);

        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings::default();

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_contour2d_with_tabs() {
        let contour = Contour::rectangle(0.0, 0.0, 50.0, 40.0);
        let op = Contour2D::new(contour, 10.0).with_tabs(4, 5.0, 2.0);

        assert_eq!(op.tabs.len(), 4);

        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 25.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepdown: 5.0,
            ..CamSettings::default()
        };

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());

        // Check that we have Z variations in final pass (tabs)
        let z_values: Vec<f64> = toolpath
            .segments
            .iter()
            .filter_map(|s| s.target())
            .map(|[_, _, z]| z)
            .collect();

        // Should have at least two distinct Z values (cut depth and tab height)
        let mut unique_z: Vec<f64> = z_values.clone();
        unique_z.sort_by(|a, b| a.total_cmp(b));
        unique_z.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        assert!(unique_z.len() >= 2);
    }

    #[test]
    fn test_contour2d_circle() {
        let contour = Contour::circle(25.0, 25.0, 15.0);
        let op = Contour2D::new(contour, 6.0);

        let tool = Tool::default_endmill();
        let settings = CamSettings::default();

        let toolpath = op.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_tab_creation() {
        let tab = Tab::new(0.25, 5.0, 2.0);
        assert!((tab.position - 0.25).abs() < 1e-6);
        assert!((tab.width - 5.0).abs() < 1e-6);
        assert!((tab.height - 2.0).abs() < 1e-6);
    }
}
