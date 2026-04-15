//! TUI mode implementations.
//!
//! Each mode provides a specialized interface for different CAD workflows:
//! - Normal: Basic geometry creation and editing
//! - Sketch: 2D constraint-based drawing (backed by vcad-kernel-constraints)
//! - Assembly: Part instances and joints
//! - Physics: Simulation and RL training
//! - CAM: Toolpath generation and G-code
//! - Print: 3D print slicing and sending to printers

pub mod modes;
pub mod sketch_mode;

pub use modes::*;

// Re-export the kernel sketch plane so call sites elsewhere in the crate
// (e.g. `crate::tui::SketchPlane::XY`) keep compiling.
pub use vcad_kernel_constraints::SketchPlane;
