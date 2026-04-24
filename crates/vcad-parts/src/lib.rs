//! Built-in parametric parts (stdlib) for vcad.
//!
//! Each part is a Rust module exporting a [`PartMetadata`] constant and a
//! `build` function. The registry is assembled at compile time; the engine
//! resolves `std:*` paths in [`vcad_ir::CsgOp::PartInstance`] by calling
//! [`build_part`].
//!
//! New parts live under `src/<category>/<slug>.rs` and are registered in
//! [`all_parts`].

#![warn(missing_docs)]

use std::collections::HashMap;

pub mod builder;
pub mod types;

pub mod bearings;
pub mod fasteners;

pub use builder::Builder;
pub use types::{BuildFn, Param, Params, PartEntry, PartMetadata, Xref};

use vcad_ir::Document;

/// All registered built-in parts.
pub fn all_parts() -> &'static [PartEntry] {
    &[
        fasteners::washer_flat::ENTRY,
        fasteners::bolt_socket_head::ENTRY,
        fasteners::nut_hex::ENTRY,
        bearings::bearing_608::ENTRY,
    ]
}

/// Look up a part by its `std:` path.
pub fn find_part(path: &str) -> Option<&'static PartEntry> {
    let stripped = path.strip_prefix("std:").unwrap_or(path);
    all_parts().iter().find(|e| e.meta.id == stripped)
}

/// Build a part's geometry given its path and params.
///
/// Returns the full sub-document the part produces. The caller is expected to
/// splice this into a parent document and keep the [`vcad_ir::CsgOp::PartInstance`]
/// node around as the edit surface.
pub fn build_part(
    path: &str,
    params: &HashMap<String, serde_json::Value>,
) -> Result<Document, String> {
    let entry = find_part(path).ok_or_else(|| format!("unknown part: {path}"))?;
    let p = Params::new(entry.meta.params, params);
    (entry.build)(&p)
}

/// Emit the manifest JSON the app uses for palette + Cmd+K indexing.
pub fn manifest_json() -> String {
    let entries: Vec<_> = all_parts().iter().map(|e| e.manifest_entry()).collect();
    serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string())
}
