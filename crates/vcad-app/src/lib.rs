//! Shared application core for vcad.
//!
//! This crate provides the `DocumentEngine` — the single source of truth for
//! document state, evaluation, undo/redo, camera, selection, and commands.
//! Both the web app (via WASM) and the Rust CLI link against this crate,
//! eliminating behavioral drift.

pub mod camera;
pub mod commands;
pub mod engine;
pub mod evaluate;
pub mod selection;

pub use camera::Camera;
pub use commands::{Command, ToolbarTab};
pub use engine::DocumentEngine;
pub use evaluate::{EvaluatedMesh, EvaluatedScene};
pub use selection::Selection;
