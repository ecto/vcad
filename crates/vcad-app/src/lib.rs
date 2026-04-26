//! Shared application core for vcad.
//!
//! This crate provides the `DocumentEngine` — the single source of truth for
//! document state, evaluation, undo/redo, camera, selection, and commands.
//! Both the web app (via WASM) and the Rust CLI link against this crate,
//! eliminating behavioral drift.

pub mod camera;
pub mod commands;
pub mod context;
pub mod document_api;
pub mod engine;
pub mod evaluate;
pub mod feature;
pub mod keybinding;
pub mod materializer;
pub mod migrate;
pub mod mode;
pub mod part_info;
pub mod registry;
pub mod selection;

pub use camera::Camera;
pub use commands::{Command, CommandCategory, ToolbarTab};
pub use context::{WhenContext, WhenExpr, WhenParseError};
pub use document_api::{ApiResult, DocumentApi, StableIdMap};
pub use engine::{DocumentEngine, FocusZone};
pub use evaluate::{EvaluatedMesh, EvaluatedScene};
pub use feature::{BooleanType, FeatureInput};
pub use keybinding::{Chord, ChordParseError, Key};
pub use materializer::{materialize, MaterializeResult};
pub use mode::{AppMode, ModeScope, Target};
pub use part_info::PartInfo;
pub use registry::{KeybindingRegistry, LoadError};
pub use selection::{Selection, SelectionFilter, SelectionItem};
