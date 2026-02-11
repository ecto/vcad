//! TUI mode implementations.
//!
//! Each mode provides a specialized interface for different CAD workflows:
//! - Normal: Basic geometry creation and editing
//! - Sketch: 2D constraint-based drawing
//! - Assembly: Part instances and joints
//! - Physics: Simulation and RL training
//! - CAM: Toolpath generation and G-code
//! - Print: 3D print slicing and sending to printers

pub mod modes;
pub mod sketch_mode;

pub use modes::*;
