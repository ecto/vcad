//! Shared application core for vcad.
//!
//! This crate provides the `DocumentEngine` — the single source of truth for
//! document state, evaluation, undo/redo, camera, selection, and commands.
//! Both the web app (via WASM) and the Rust CLI link against this crate,
//! eliminating behavioral drift.

pub mod camera;
pub mod commands;
pub mod document_api;
pub mod engine;
pub mod evaluate;
pub mod feature;
pub mod materializer;
pub mod migrate;
pub mod part_info;
pub mod selection;

pub use camera::Camera;
pub use commands::{Command, ToolbarTab};
pub use document_api::{ApiResult, DocumentApi, StableIdMap};
pub use engine::DocumentEngine;
pub use evaluate::{EvaluatedMesh, EvaluatedScene};
pub use feature::{BooleanType, FeatureInput};
pub use materializer::{materialize, MaterializeResult};
pub use part_info::PartInfo;
pub use selection::Selection;
