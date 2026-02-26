//! 2D pocket clearing operation.

use crate::operation::Contour;
use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
#[cfg(not(target_arch = "wasm32"))]
use geo::algorithm::centroid::Centroid;
#[cfg(not(target_arch = "wasm32"))]
use geo_clipper::Clipper;
use serde::{Deserialize, Serialize};

/// 2D pocket clearing operation.
///
/// Clears material inside a closed contour using concentric offset passes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pocket2D {
    /// The outer boundary contour.
    pub contour: Contour,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
    /// Stock to leave on walls (finish allowance).
    pub stock_to_leave: f64,
}

impl Pocket2D {
    /// Create a new pocket operation.
    pub fn new(contour: Contour, depth: f64) -> Self {
        Self {
            contour,
            depth,
            stock_to_leave: 0.0,
        }
    }

    /// Set stock to leave on walls.
    pub fn with_stock_to_leave(mut self, stock: f64) -> Self {
        self.stock_to_leave = stock;
        self
    }

    /// Create a rectangular pocket.
    pub fn rectangle(x: f64, y: f64, width: f64, height: f64, depth: f64) -> Self {
        Self::new(Contour::rectangle(x, y, width, height), depth)
    }

    /// Create a circular pocket.
    pub fn circle(cx: f64, cy: f64, radius: f64, depth: f64) -> Self {
        Self::new(Contour::circle(cx, cy, radius), depth)
    }

    /// Generate the toolpath for this pocket operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        // Validate inputs
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepover <= 0.0 || settings.stepover > tool.diameter() {
            return Err(CamError::InvalidStepover(settings.stepover));
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

        let mut toolpath = Toolpath::new();

        toolpath.push(ToolpathSegment::comment(format!(
            "Pocket 2D: depth={:.2}mm, stock_to_leave={:.2}mm",
            self.depth, self.stock_to_leave
        )));

        let tool_radius = tool.radius();
        let stepover = settings.stepover;

        // Calculate total inward offset (tool radius + stock to leave)
        let total_offset = tool_radius + self.stock_to_leave;

        // Convert contour to geo polygon
        let polygon = self.contour.to_geo_polygon();

        // Generate offset rings from outside to inside
        let offset_rings = self.generate_offset_rings(&polygon, total_offset, stepover)?;

        if offset_rings.is_empty() {
            return Err(CamError::EmptyPocketOffset);
        }

        // Calculate Z levels
        let num_z_passes = (self.depth / settings.stepdown).ceil() as usize;
        let z_step = self.depth / num_z_passes as f64;

        for z_pass in 0..num_z_passes {
            let z = -((z_pass + 1) as f64) * z_step;

            toolpath.push(ToolpathSegment::comment(format!("Z level: {:.3}", z)));

            // Machine from inside out (climb milling direction)
            for (ring_idx, ring) in offset_rings.iter().rev().enumerate() {
                if ring.0.is_empty() {
                    continue;
                }

                let points = &ring.0;

                if ring_idx == 0 {
                    // First ring: plunge at center
                    let start = &points[0];
                    toolpath.push(ToolpathSegment::rapid(start.x, start.y, settings.safe_z));
                    toolpath.push(ToolpathSegment::linear(
                        start.x,
                        start.y,
                        z,
                        settings.plunge_rate,
                    ));
                } else {
                    // Move to start of this ring
                    let start = &points[0];
                    toolpath.push(ToolpathSegment::linear(
                        start.x,
                        start.y,
                        z,
                        settings.feed_rate,
                    ));
                }

                // Follow the ring
                for point in points.iter().skip(1) {
                    toolpath.push(ToolpathSegment::linear(
                        point.x,
                        point.y,
                        z,
                        settings.feed_rate,
                    ));
                }

                // Close the ring
                let first = &points[0];
                toolpath.push(ToolpathSegment::linear(
                    first.x,
                    first.y,
                    z,
                    settings.feed_rate,
                ));
            }

            // Retract between Z levels
            if z_pass < num_z_passes - 1 {
                if let Some([x, y, _]) = toolpath.segments.last().and_then(|s| s.target()) {
                    toolpath.push(ToolpathSegment::rapid(x, y, settings.safe_z));
                }
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

    /// Generate concentric offset rings for the pocket (native version with geo-clipper).
    #[cfg(not(target_arch = "wasm32"))]
    fn generate_offset_rings(
        &self,
        polygon: &geo::Polygon<f64>,
        initial_offset: f64,
        stepover: f64,
    ) -> Result<Vec<geo::LineString<f64>>, CamError> {
        let mut rings = Vec::new();
        let mut current_offset = initial_offset;

        // Scale factor for geo-clipper (uses integer math)
        let scale = 1000.0;

        loop {
            // Offset inward (negative value)
            let offset_distance = -current_offset * scale;

            let result = polygon.offset(
                offset_distance,
                geo_clipper::JoinType::Round(10.0),
                geo_clipper::EndType::ClosedPolygon,
                scale,
            );

            if result.0.is_empty() {
                break;
            }

            // Take the exterior of the first polygon
            if let Some(poly) = result.0.first() {
                let exterior = poly.exterior().clone();
                if exterior.0.len() >= 3 {
                    rings.push(exterior);
                } else {
                    break;
                }
            } else {
                break;
            }

            current_offset += stepover;
        }

        // If we have no rings but we should, start from center
        if rings.is_empty() {
            if let Some(centroid) = polygon.centroid() {
                let center_ring = geo::LineString::from(vec![geo::Coord {
                    x: centroid.x(),
                    y: centroid.y(),
                }]);
                rings.push(center_ring);
            }
        }

        Ok(rings)
    }

    /// Generate concentric offset rings for the pocket (WASM version with simple offset).
    ///
    /// This is a simplified implementation that works for rectangular and circular pockets.
    /// For complex shapes, a more sophisticated algorithm would be needed.
    #[cfg(target_arch = "wasm32")]
    fn generate_offset_rings(
        &self,
        polygon: &geo::Polygon<f64>,
        initial_offset: f64,
        stepover: f64,
    ) -> Result<Vec<geo::LineString<f64>>, CamError> {
        use geo::BoundingRect;

        let mut rings = Vec::new();
        let mut current_offset = initial_offset;

        // Get bounding box to determine pocket extents
        let Some(bbox) = polygon.bounding_rect() else {
            return Err(CamError::EmptyPocketOffset);
        };

        let width = bbox.width();
        let height = bbox.height();
        let cx = bbox.min().x + width / 2.0;
        let cy = bbox.min().y + height / 2.0;

        // Determine if this is roughly circular or rectangular
        let is_circular = self.contour.is_circular();

        if is_circular {
            // For circular pockets, generate concentric circles
            let radius = width.min(height) / 2.0;
            while current_offset < radius {
                let r = radius - current_offset;
                if r <= 0.0 {
                    break;
                }

                // Generate circle points
                let segments = 36;
                let points: Vec<geo::Coord<f64>> = (0..=segments)
                    .map(|i| {
                        let angle = 2.0 * std::f64::consts::PI * (i as f64) / (segments as f64);
                        geo::Coord {
                            x: cx + r * angle.cos(),
                            y: cy + r * angle.sin(),
                        }
                    })
                    .collect();
                rings.push(geo::LineString::from(points));
                current_offset += stepover;
            }
        } else {
            // For rectangular pockets, generate concentric rectangles
            while current_offset < width.min(height) / 2.0 {
                let inset_w = width / 2.0 - current_offset;
                let inset_h = height / 2.0 - current_offset;

                if inset_w <= 0.0 || inset_h <= 0.0 {
                    break;
                }

                let points = vec![
                    geo::Coord {
                        x: cx - inset_w,
                        y: cy - inset_h,
                    },
                    geo::Coord {
                        x: cx + inset_w,
                        y: cy - inset_h,
                    },
                    geo::Coord {
                        x: cx + inset_w,
                        y: cy + inset_h,
                    },
                    geo::Coord {
                        x: cx - inset_w,
                        y: cy + inset_h,
                    },
                    geo::Coord {
                        x: cx - inset_w,
                        y: cy - inset_h,
                    },
                ];
                rings.push(geo::LineString::from(points));
                current_offset += stepover;
            }
        }

        // If we have no rings, add center point
        if rings.is_empty() {
            let center_ring = geo::LineString::from(vec![geo::Coord { x: cx, y: cy }]);
            rings.push(center_ring);
        }

        Ok(rings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pocket_rectangle() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, 5.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepover: 3.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        };

        let toolpath = pocket.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());

        // Should have multiple Z passes
        let z_comments = toolpath
            .segments
            .iter()
            .filter(|s| matches!(s, ToolpathSegment::Comment { text } if text.contains("Z level")))
            .count();
        assert!(z_comments >= 2);
    }

    #[test]
    fn test_pocket_circle() {
        let pocket = Pocket2D::circle(25.0, 25.0, 10.0, 3.0);
        let tool = Tool::FlatEndMill {
            diameter: 4.0,
            flute_length: 15.0,
            flutes: 2,
        };
        let settings = CamSettings::default();

        let toolpath = pocket.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());
    }

    #[test]
    fn test_pocket_with_stock_to_leave() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, 5.0).with_stock_to_leave(0.5);
        assert!((pocket.stock_to_leave - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_pocket_invalid_depth() {
        let pocket = Pocket2D::rectangle(0.0, 0.0, 20.0, 15.0, -1.0);
        let tool = Tool::default_endmill();
        let settings = CamSettings::default();

        let result = pocket.generate(&tool, &settings);
        assert!(matches!(result, Err(CamError::InvalidDepth(_))));
    }
}
