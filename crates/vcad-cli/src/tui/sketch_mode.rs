//! Sketch mode wiring.
//!
//! The TUI delegates all sketching logic — tool state, snapping, hit-testing,
//! shape building, constraint solving, undo/redo — to the kernel
//! [`vcad_kernel_constraints::SketchSession`]. This file is the thin glue
//! that maps TUI keystrokes onto session methods so the web app and the TUI
//! drive the same code.

#![allow(dead_code)] // Public API surface reused by external call sites and tests.

use crate::tui::modes::SketchModeState;
use vcad_ir::{CsgOp, SketchSegment2D, Vec2, Vec3};
use vcad_kernel_constraints::{SegmentKind, SegmentView, SketchTool};

impl SketchModeState {
    /// Handle a character key in sketch mode. Returns true if the key was
    /// consumed.
    pub fn handle_key(&mut self, key: char) -> bool {
        match key.to_ascii_lowercase() {
            's' => {
                self.session.set_tool(SketchTool::Select);
                true
            }
            'l' => {
                self.session.set_tool(SketchTool::Line);
                true
            }
            'r' => {
                self.session.set_tool(SketchTool::Rectangle);
                true
            }
            'c' => {
                self.session.set_tool(SketchTool::Circle);
                true
            }
            'a' => {
                self.session.set_tool(SketchTool::Arc);
                true
            }
            'p' => {
                self.session.set_tool(SketchTool::Point);
                true
            }
            'u' => self.session.undo(),
            'z' => self.session.redo(),
            _ => false,
        }
    }

    /// Human-readable name of the current tool.
    pub fn tool_name(&self) -> &'static str {
        match self.session.tool() {
            SketchTool::Select => "Select",
            SketchTool::Line => "Line",
            SketchTool::Rectangle => "Rectangle",
            SketchTool::Circle => "Circle",
            SketchTool::Arc => "Arc",
            SketchTool::Point => "Point",
        }
    }

    /// Origin + local X/Y axes of the sketch plane as plain arrays.
    pub fn plane_axes(&self) -> ([f64; 3], [f64; 3], [f64; 3]) {
        let plane = self.session.plane();
        (plane.origin(), plane.x_dir(), plane.y_dir())
    }

    /// Convert the current sketch into a `CsgOp::Sketch2D` node. Returns
    /// `None` if the session has no drawable segments.
    pub fn to_sketch2d_op(&self) -> Option<CsgOp> {
        let views = self.session.segments();
        if views.is_empty() {
            return None;
        }
        let segments: Vec<SketchSegment2D> = views.iter().flat_map(segment_view_to_ir).collect();
        let plane = self.session.plane();
        let o = plane.origin();
        let x = plane.x_dir();
        let y = plane.y_dir();
        Some(CsgOp::Sketch2D {
            origin: Vec3::new(o[0], o[1], o[2]),
            x_dir: Vec3::new(x[0], x[1], x[2]),
            y_dir: Vec3::new(y[0], y[1], y[2]),
            segments,
        })
    }
}

/// Convert a session [`SegmentView`] into one or more IR segments. Circles
/// are emitted as four quarter-arcs for compatibility with the existing IR.
fn segment_view_to_ir(view: &SegmentView) -> Vec<SketchSegment2D> {
    match view.kind {
        SegmentKind::Line => vec![SketchSegment2D::Line {
            start: Vec2::new(view.start[0], view.start[1]),
            end: Vec2::new(view.end[0], view.end[1]),
        }],
        SegmentKind::Arc { center, ccw } => vec![SketchSegment2D::Arc {
            start: Vec2::new(view.start[0], view.start[1]),
            end: Vec2::new(view.end[0], view.end[1]),
            center: Vec2::new(center[0], center[1]),
            ccw,
        }],
        SegmentKind::Circle { center, radius } => {
            let n = 4;
            (0..n)
                .map(|i| {
                    let a0 = (i as f64) * std::f64::consts::TAU / (n as f64);
                    let a1 = ((i + 1) as f64) * std::f64::consts::TAU / (n as f64);
                    let start = [center[0] + radius * a0.cos(), center[1] + radius * a0.sin()];
                    let end = [center[0] + radius * a1.cos(), center[1] + radius * a1.sin()];
                    SketchSegment2D::Arc {
                        start: Vec2::new(start[0], start[1]),
                        end: Vec2::new(end[0], end[1]),
                        center: Vec2::new(center[0], center[1]),
                        ccw: true,
                    }
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::modes::SketchModeState;
    use vcad_kernel_constraints::SketchPlane;

    #[test]
    fn rectangle_via_tool_builds_sketch2d_op() {
        let mut state = SketchModeState::new(SketchPlane::XY);
        state.session.set_tool(SketchTool::Rectangle);
        state.session.on_cursor_sketch(0.0, 0.0);
        state.session.on_click();
        state.session.on_cursor_sketch(10.0, 5.0);
        state.session.on_click();
        let op = state.to_sketch2d_op().expect("sketch op");
        match op {
            CsgOp::Sketch2D { segments, .. } => assert_eq!(segments.len(), 4),
            _ => panic!("expected Sketch2D"),
        }
    }

    #[test]
    fn tool_switch_via_key() {
        let mut state = SketchModeState::new(SketchPlane::XY);
        assert_eq!(state.session.tool(), SketchTool::Select);
        assert!(state.handle_key('l'));
        assert_eq!(state.session.tool(), SketchTool::Line);
        assert!(state.handle_key('r'));
        assert_eq!(state.session.tool(), SketchTool::Rectangle);
    }
}
