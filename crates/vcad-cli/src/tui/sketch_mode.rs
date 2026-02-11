//! Sketch mode implementation.
//!
//! Provides 2D constraint-based sketching within the TUI.

#![allow(dead_code)] // Will be used as sketch mode is implemented

use crate::tui::modes::{SketchModeState, SketchPlane, SketchTool};
use vcad_ir::{CsgOp, NodeId, SketchSegment2D, Vec2, Vec3};

impl SketchModeState {
    /// Create a new sketch mode on the given plane.
    pub fn new(plane: SketchPlane) -> Self {
        Self {
            tool: SketchTool::Select,
            plane,
            selected_entities: Vec::new(),
            cursor: [0.0, 0.0],
            pending_start: None,
            target_face: None,
        }
    }

    /// Handle key input in sketch mode.
    /// Returns true if the key was handled.
    pub fn handle_key(&mut self, key: char) -> bool {
        match key.to_ascii_lowercase() {
            'l' => {
                self.tool = SketchTool::Line;
                self.pending_start = None;
                true
            }
            'r' => {
                self.tool = SketchTool::Rectangle;
                self.pending_start = None;
                true
            }
            'c' => {
                self.tool = SketchTool::Circle;
                self.pending_start = None;
                true
            }
            'a' => {
                self.tool = SketchTool::Arc;
                self.pending_start = None;
                true
            }
            'p' => {
                self.tool = SketchTool::Point;
                true
            }
            _ => false,
        }
    }

    /// Get the current tool name.
    pub fn tool_name(&self) -> &'static str {
        match self.tool {
            SketchTool::Select => "Select",
            SketchTool::Line => "Line",
            SketchTool::Rectangle => "Rectangle",
            SketchTool::Circle => "Circle",
            SketchTool::Arc => "Arc",
            SketchTool::Point => "Point",
        }
    }

    /// Get the plane origin and axes.
    pub fn plane_axes(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        match self.plane {
            SketchPlane::XY => ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            SketchPlane::XZ => ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            SketchPlane::YZ => ([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            SketchPlane::Custom { origin, normal } => {
                // Compute orthonormal basis from normal
                let n = normalize(normal);
                let up = if n[1].abs() < 0.9 {
                    [0.0, 1.0, 0.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let x = cross(up, n);
                let x = normalize(x);
                let y = cross(n, x);
                (origin, x, y)
            }
        }
    }
}

/// Simple sketch builder for generating extrusion geometry.
#[derive(Debug, Clone, Default)]
pub struct SketchBuilder {
    segments: Vec<SketchSegment2D>,
}

impl SketchBuilder {
    /// Create a new empty sketch builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a line segment.
    pub fn add_line(&mut self, start: [f64; 2], end: [f64; 2]) {
        self.segments.push(SketchSegment2D::Line {
            start: Vec2::new(start[0], start[1]),
            end: Vec2::new(end[0], end[1]),
        });
    }

    /// Add a rectangle (4 line segments forming a closed loop).
    pub fn add_rectangle(&mut self, x: f64, y: f64, width: f64, height: f64) {
        let corners = [
            [x, y],
            [x + width, y],
            [x + width, y + height],
            [x, y + height],
        ];

        for i in 0..4 {
            let start = corners[i];
            let end = corners[(i + 1) % 4];
            self.add_line(start, end);
        }
    }

    /// Add a circle approximation (as multiple arc segments).
    pub fn add_circle(&mut self, center: [f64; 2], radius: f64) {
        // Approximate circle with 4 quarter arcs
        let n = 4;
        for i in 0..n {
            let angle_start = (i as f64) * std::f64::consts::TAU / (n as f64);
            let angle_end = ((i + 1) as f64) * std::f64::consts::TAU / (n as f64);

            let start = [
                center[0] + radius * angle_start.cos(),
                center[1] + radius * angle_start.sin(),
            ];
            let end = [
                center[0] + radius * angle_end.cos(),
                center[1] + radius * angle_end.sin(),
            ];

            self.segments.push(SketchSegment2D::Arc {
                start: Vec2::new(start[0], start[1]),
                end: Vec2::new(end[0], end[1]),
                center: Vec2::new(center[0], center[1]),
                ccw: true,
            });
        }
    }

    /// Build a Sketch2D node from the sketch.
    pub fn build_sketch2d(&self, plane: SketchPlane) -> CsgOp {
        let (origin, x_dir, y_dir) = match plane {
            SketchPlane::XY => (
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
            ),
            SketchPlane::XZ => (
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ),
            SketchPlane::YZ => (
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
            ),
            SketchPlane::Custom { origin, normal } => {
                let n = normalize(normal);
                let up = if n[1].abs() < 0.9 {
                    [0.0, 1.0, 0.0]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let x = cross(up, n);
                let x = normalize(x);
                let y = cross(n, x);
                (
                    Vec3::new(origin[0], origin[1], origin[2]),
                    Vec3::new(x[0], x[1], x[2]),
                    Vec3::new(y[0], y[1], y[2]),
                )
            }
        };

        CsgOp::Sketch2D {
            origin,
            x_dir,
            y_dir,
            segments: self.segments.clone(),
        }
    }

    /// Build an extrude operation from the sketch.
    ///
    /// Returns (sketch_op, extrude_op) - both need to be added as nodes.
    pub fn build_extrude(&self, plane: SketchPlane, height: f64, sketch_node_id: NodeId) -> CsgOp {
        // Compute extrusion direction (normal to plane)
        let direction = match plane {
            SketchPlane::XY => Vec3::new(0.0, 0.0, height),
            SketchPlane::XZ => Vec3::new(0.0, height, 0.0),
            SketchPlane::YZ => Vec3::new(height, 0.0, 0.0),
            SketchPlane::Custom { normal, .. } => {
                let n = normalize(normal);
                Vec3::new(n[0] * height, n[1] * height, n[2] * height)
            }
        };

        CsgOp::Extrude {
            sketch: sketch_node_id,
            direction,
            twist_angle: None,
            scale_end: None,
        }
    }

    /// Build a revolve operation from the sketch.
    pub fn build_revolve(
        &self,
        axis_origin: [f64; 3],
        axis_dir: [f64; 3],
        angle_deg: f64,
        sketch_node_id: NodeId,
    ) -> CsgOp {
        CsgOp::Revolve {
            sketch: sketch_node_id,
            axis_origin: Vec3::new(axis_origin[0], axis_origin[1], axis_origin[2]),
            axis_dir: Vec3::new(axis_dir[0], axis_dir[1], axis_dir[2]),
            angle_deg,
        }
    }

    /// Get the number of segments.
    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// Check if the sketch is empty.
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

// Vector helpers
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sketch_builder_rectangle() {
        let mut builder = SketchBuilder::new();
        builder.add_rectangle(0.0, 0.0, 10.0, 5.0);
        assert_eq!(builder.len(), 4);
    }

    #[test]
    fn test_sketch_builder_circle() {
        let mut builder = SketchBuilder::new();
        builder.add_circle([0.0, 0.0], 5.0);
        assert_eq!(builder.len(), 4); // 4 quarter arcs
    }

    #[test]
    fn test_sketch_mode_tool_switch() {
        let mut state = SketchModeState::new(SketchPlane::XY);
        assert_eq!(state.tool, SketchTool::Select);

        assert!(state.handle_key('l'));
        assert_eq!(state.tool, SketchTool::Line);

        assert!(state.handle_key('r'));
        assert_eq!(state.tool, SketchTool::Rectangle);
    }
}
