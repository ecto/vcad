//! WASM bindings for semantic diff & three-way merge of `.vcad` documents.
//!
//! Thin string-in / JSON-out wrappers over [`vcad_diff`]: the app's version
//! timeline diffs Supabase `document_versions` against their parents and
//! drives branch/merge-back flows through these entry points.

use serde::Serialize;
use vcad_diff::{DocumentDiff, MergeResult, Resolution};
use vcad_ir::Document;
use wasm_bindgen::prelude::*;

fn parse_doc(json: &str, label: &str) -> Result<Document, JsError> {
    serde_json::from_str(json).map_err(|e| JsError::new(&format!("{label} document: {e}")))
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    value
        .serialize(&ser)
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Outcome of a (possibly resolved) three-way merge, in wire form.
/// Fail-closed like the kernel: `merged` and `conflicts` are mutually
/// exclusive.
#[derive(Serialize)]
struct MergeOutcome {
    /// The merged document when every change reconciled.
    #[serde(skip_serializing_if = "Option::is_none")]
    merged: Option<Document>,
    /// Unresolved conflicts; present (non-empty) when the merge failed closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    conflicts: Option<Vec<vcad_diff::Conflict>>,
}

impl From<MergeResult> for MergeOutcome {
    fn from(r: MergeResult) -> Self {
        match r {
            MergeResult::Merged(doc) => Self {
                merged: Some(*doc),
                conflicts: None,
            },
            MergeResult::Conflicts(cs) => Self {
                merged: None,
                conflicts: Some(cs),
            },
        }
    }
}

/// Semantic (entity-level) diff of two `.vcad` documents.
///
/// Returns a `DocumentDiff` JSON value: `{ changes: [{ kind, id, name?,
/// type: "added"|"removed"|"modified", value?|fields? }] }`. Entities are
/// matched by stable id, so reordering alone yields an empty diff.
#[wasm_bindgen(js_name = documentDiff)]
pub fn document_diff(old_json: &str, new_json: &str) -> Result<JsValue, JsError> {
    let old = parse_doc(old_json, "old")?;
    let new = parse_doc(new_json, "new")?;
    let diff = vcad_diff::diff(&old, &new).map_err(|e| JsError::new(&e.to_string()))?;
    to_js(&diff)
}

/// Apply a `DocumentDiff` (as produced by [`document_diff`]) to a document,
/// returning the patched document JSON.
#[wasm_bindgen(js_name = documentDiffApply)]
pub fn document_diff_apply(old_json: &str, diff_json: &str) -> Result<JsValue, JsError> {
    let old = parse_doc(old_json, "old")?;
    let diff: DocumentDiff =
        serde_json::from_str(diff_json).map_err(|e| JsError::new(&format!("diff: {e}")))?;
    let patched = vcad_diff::apply(&old, &diff).map_err(|e| JsError::new(&e.to_string()))?;
    to_js(&patched)
}

/// Fail-closed three-way merge of two documents against a common ancestor.
///
/// `resolutions_json` is an optional JSON array of user decisions
/// (`[{ kind, id, path?, side: "ours"|"theirs" }]`) settling previously
/// reported conflicts; pass `null`/empty for a plain merge. Returns
/// `{ merged }` on success or `{ conflicts }` when unresolved conflicts
/// remain — never both.
#[wasm_bindgen(js_name = documentMerge)]
pub fn document_merge(
    base_json: &str,
    ours_json: &str,
    theirs_json: &str,
    resolutions_json: Option<String>,
) -> Result<JsValue, JsError> {
    let base = parse_doc(base_json, "base")?;
    let ours = parse_doc(ours_json, "ours")?;
    let theirs = parse_doc(theirs_json, "theirs")?;
    let resolutions: Vec<Resolution> = match resolutions_json.as_deref() {
        None | Some("") | Some("null") => Vec::new(),
        Some(json) => {
            serde_json::from_str(json).map_err(|e| JsError::new(&format!("resolutions: {e}")))?
        }
    };
    let result = vcad_diff::merge_with_resolutions(&base, &ours, &theirs, &resolutions)
        .map_err(|e| JsError::new(&e.to_string()))?;
    to_js(&MergeOutcome::from(result))
}

/// Human-readable one-line-per-change rendering of a `DocumentDiff`.
#[wasm_bindgen(js_name = documentDiffHuman)]
pub fn document_diff_human(diff_json: &str) -> Result<String, JsError> {
    let diff: DocumentDiff =
        serde_json::from_str(diff_json).map_err(|e| JsError::new(&format!("diff: {e}")))?;
    Ok(vcad_diff::render_human(&diff))
}
