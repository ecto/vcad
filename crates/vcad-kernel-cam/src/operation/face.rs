//! Face (surface) milling operation.

use crate::{CamError, CamSettings, Tool, Toolpath, ToolpathSegment};
use serde::{Deserialize, Serialize};

/// Face milling operation for surface machining.
///
/// Generates a raster (zigzag) pattern to machine a flat surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Face {
    /// Minimum X coordinate of the area to face.
    pub min_x: f64,
    /// Minimum Y coordinate of the area to face.
    pub min_y: f64,
    /// Maximum X coordinate of the area to face.
    pub max_x: f64,
    /// Maximum Y coordinate of the area to face.
    pub max_y: f64,
    /// Depth to cut (positive value, measured from Z=0).
    pub depth: f64,
}

impl Face {
    /// Create a new face operation.
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64, depth: f64) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
            depth,
        }
    }

    /// Create a face operation from width and height at origin.
    pub fn from_size(width: f64, height: f64, depth: f64) -> Self {
        Self::new(0.0, 0.0, width, height, depth)
    }

    /// Generate the toolpath for this face operation.
    pub fn generate(&self, tool: &Tool, settings: &CamSettings) -> Result<Toolpath, CamError> {
        // Validate inputs
        let width = self.max_x - self.min_x;
        let height = self.max_y - self.min_y;

        if width <= 0.0 || height <= 0.0 {
            return Err(CamError::DegenerateBounds(width, height));
        }
        if self.depth <= 0.0 {
            return Err(CamError::InvalidDepth(self.depth));
        }
        if settings.stepover <= 0.0 || settings.stepover > tool.diameter() {
            return Err(CamError::InvalidStepover(settings.stepover));
        }
        if settings.feed_rate <= 0.0 {
            return Err(CamError::InvalidFeedRate(settings.feed_rate));
        }

        let mut toolpath = Toolpath::new();

        // Add operation comment
        toolpath.push(ToolpathSegment::comment(format!(
            "Face: {:.1}x{:.1}mm, depth={:.2}mm",
            width, height, self.depth
        )));

        let tool_radius = tool.radius();
        let stepover = settings.stepover;

        // Expand bounds by tool radius to ensure full coverage
        let start_x = self.min_x - tool_radius;
        let end_x = self.max_x + tool_radius;
        let start_y = self.min_y - tool_radius;
        let end_y = self.max_y + tool_radius;

        // Calculate Z levels
        let num_z_passes = (self.depth / settings.stepdown).ceil() as usize;
        let z_step = self.depth / num_z_passes as f64;

        for z_pass in 0..num_z_passes {
            let z = -((z_pass + 1) as f64) * z_step;

            // Calculate Y passes
            let y_span = end_y - start_y;
            let num_passes = (y_span / stepover).ceil() as usize;
            let actual_stepover = y_span / num_passes as f64;

            toolpath.push(ToolpathSegment::comment(format!("Z level: {:.3}", z)));

            for pass in 0..=num_passes {
                let y = start_y + pass as f64 * actual_stepover;
                let y = y.min(end_y);

                // Alternate direction for zigzag pattern
                let (x_start, x_end) = if pass % 2 == 0 {
                    (start_x, end_x)
                } else {
                    (end_x, start_x)
                };

                if pass == 0 {
                    // First pass: rapid to start position at safe Z, then plunge
                    toolpath.push(ToolpathSegment::rapid(x_start, y, settings.safe_z));
                    toolpath.push(ToolpathSegment::linear(x_start, y, z, settings.plunge_rate));
                } else {
                    // Step over to next row
                    toolpath.push(ToolpathSegment::linear(x_start, y, z, settings.feed_rate));
                }

                // Cut across
                toolpath.push(ToolpathSegment::linear(x_end, y, z, settings.feed_rate));
            }

            // Retract after each Z level
            if z_pass < num_z_passes - 1 {
                let last_pos = toolpath.segments.last().and_then(|s| s.target());
                if let Some([x, y, _]) = last_pos {
                    toolpath.push(ToolpathSegment::rapid(x, y, settings.safe_z));
                }
            }
        }

        // Final retract
        toolpath.push(ToolpathSegment::rapid(
            self.min_x,
            self.min_y,
            settings.safe_z,
        ));

        Ok(toolpath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_face_basic() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 2.0);
        let tool = Tool::FlatEndMill {
            diameter: 10.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let settings = CamSettings {
            stepover: 5.0,
            stepdown: 2.0,
            feed_rate: 1000.0,
            plunge_rate: 300.0,
            spindle_rpm: 12000.0,
            safe_z: 5.0,
            retract_z: 10.0,
        };

        let toolpath = face.generate(&tool, &settings).unwrap();
        assert!(!toolpath.is_empty());

        // Check that we have cutting moves
        let cutting_moves = toolpath.segments.iter().filter(|s| s.is_cutting()).count();
        assert!(cutting_moves > 0);

        // Check Z depth is correct
        let min_z = toolpath
            .segments
            .iter()
            .filter_map(|s| s.target())
            .map(|[_, _, z]| z)
            .fold(f64::INFINITY, f64::min);
        assert!((min_z - -2.0).abs() < 0.01);
    }

    #[test]
    fn test_face_from_size() {
        let face = Face::from_size(100.0, 50.0, 1.0);
        assert!((face.min_x - 0.0).abs() < 1e-6);
        assert!((face.max_x - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_face_invalid_bounds() {
        let face = Face::new(10.0, 10.0, 5.0, 5.0, 1.0); // Invalid: max < min
        let tool = Tool::default_endmill();
        let settings = CamSettings::default();

        let result = face.generate(&tool, &settings);
        assert!(matches!(result, Err(CamError::DegenerateBounds(_, _))));
    }

    #[test]
    fn test_face_invalid_stepover() {
        let face = Face::new(0.0, 0.0, 50.0, 30.0, 1.0);
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let mut settings = CamSettings::default();
        settings.stepover = 10.0; // Larger than tool diameter

        let result = face.generate(&tool, &settings);
        assert!(matches!(result, Err(CamError::InvalidStepover(_))));
    }
}
