//! WASM bindings for the CRDT document engine.
//!
//! Exposes `WasmDocumentEngine` for browser/WASM consumers. Each mutation
//! returns a JSON object with `{ document, parts, changeSet, createdFeatureId? }`.

use std::collections::HashMap;

use vcad_app::materializer::materialize;
use vcad_app::migrate::{detect_format, migrate_v1, FileFormat};
use vcad_crdt::{CrdtDocument, FeatureId, FractionalIndex, ReplicaId, Value};
use wasm_bindgen::prelude::*;

/// CRDT-backed document engine for WASM.
///
/// Wraps a `CrdtDocument` and maintains cached materialized state.
/// All mutations return the updated document + parts as a JS value.
#[wasm_bindgen]
pub struct WasmDocumentEngine {
    crdt: CrdtDocument,
}

#[wasm_bindgen]
impl WasmDocumentEngine {
    /// Create a new empty document engine.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        // Use timestamp-based replica ID for uniqueness.
        let replica_id = ReplicaId(js_sys::Date::now() as u64);
        Self {
            crdt: CrdtDocument::new(replica_id),
        }
    }

    /// Create a feature with the given kind and params (JSON string).
    ///
    /// Returns `{ document, parts, createdFeatureId }` as a JsValue.
    pub fn create_feature(&mut self, kind: &str, params_json: &str) -> JsValue {
        let params: HashMap<String, Value> =
            serde_json::from_str(params_json).unwrap_or_default();

        // Find position at end of feature list.
        let ordered = self.crdt.ordered_features();
        let position = if let Some(last) = ordered.last() {
            FractionalIndex::between(Some(&last.1.position), None)
        } else {
            FractionalIndex::between(None, None)
        };

        let (fid, _cs) = self.crdt.create_feature(kind, position, params);
        self.build_result(Some(fid))
    }

    /// Delete a feature by ID (JSON string).
    pub fn delete_feature(&mut self, feature_id_json: &str) -> JsValue {
        if let Some(fid) = parse_feature_id(feature_id_json) {
            self.crdt.delete_feature(fid);
        }
        self.build_result(None)
    }

    /// Set a parameter on a feature.
    pub fn set_param(
        &mut self,
        feature_id_json: &str,
        key: &str,
        value_json: &str,
    ) -> JsValue {
        if let (Some(fid), Ok(value)) = (
            parse_feature_id(feature_id_json),
            serde_json::from_str::<Value>(value_json),
        ) {
            self.crdt.set_param(fid, key, value);
        }
        self.build_result(None)
    }

    /// Move a feature to a new position.
    pub fn move_feature(&mut self, feature_id_json: &str, position_json: &str) -> JsValue {
        if let (Some(fid), Ok(pos)) = (
            parse_feature_id(feature_id_json),
            serde_json::from_str::<FractionalIndex>(position_json),
        ) {
            self.crdt.move_feature(fid, pos);
        }
        self.build_result(None)
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> JsValue {
        self.crdt.undo();
        self.build_result(None)
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) -> JsValue {
        self.crdt.redo();
        self.build_result(None)
    }

    /// Whether undo is available.
    pub fn can_undo(&self) -> bool {
        self.crdt.can_undo()
    }

    /// Whether redo is available.
    pub fn can_redo(&self) -> bool {
        self.crdt.can_redo()
    }

    /// Get the materialized document as JSON.
    pub fn get_document_json(&self) -> String {
        let result = materialize(&self.crdt);
        serde_json::to_string(&result.document).unwrap_or_else(|_| "{}".to_string())
    }

    /// Get the parts list as JSON.
    pub fn get_parts_json(&self) -> String {
        let result = materialize(&self.crdt);
        serde_json::to_string(&result.parts).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get ordered features (for the feature tree) as JSON.
    pub fn get_ordered_features_json(&self) -> String {
        let features: Vec<_> = self
            .crdt
            .ordered_features()
            .into_iter()
            .map(|(fid, f)| {
                serde_json::json!({
                    "id": format!("{}:{}", fid.0.0, fid.1),
                    "kind": f.kind,
                    "position": f.position,
                    "params": f.params.iter().map(|(k, (v, _))| (k.clone(), v.clone())).collect::<HashMap<String, Value>>(),
                })
            })
            .collect();
        serde_json::to_string(&features).unwrap_or_else(|_| "[]".to_string())
    }

    /// Save the document to bytes.
    pub fn save(&self) -> Vec<u8> {
        self.crdt.save()
    }

    /// Load a document from bytes.
    ///
    /// Auto-detects format: if CRDT (v2), loads directly; if legacy JSON (v1),
    /// migrates to CRDT first.
    pub fn load(bytes: &[u8]) -> Result<WasmDocumentEngine, JsError> {
        match detect_format(bytes) {
            FileFormat::V2Crdt => {
                let crdt =
                    CrdtDocument::load(bytes).map_err(|e| JsError::new(&e.to_string()))?;
                Ok(Self { crdt })
            }
            FileFormat::V1Json => Self::from_v1_bytes(bytes),
            FileFormat::Unknown => Err(JsError::new("Unknown file format")),
        }
    }

    /// Load a legacy v1 JSON document and migrate to CRDT.
    pub fn from_v1_json(json: &str) -> Result<WasmDocumentEngine, JsError> {
        Self::from_v1_bytes(json.as_bytes())
    }

    /// Import IR JSON into the current document (e.g. AI-generated geometry).
    ///
    /// Parses the IR, migrates it to CRDT features, and merges the ops into
    /// this document. Returns the standard mutation result.
    pub fn import_ir(&mut self, ir_json: &str) -> JsValue {
        let doc: vcad_ir::Document = match serde_json::from_str(ir_json) {
            Ok(d) => d,
            Err(_) => return self.build_result(None),
        };
        let migrated = migrate_v1(&doc);
        // Extract all ops from the migrated doc (empty clock = all ops).
        let ops = migrated.ops_since(&std::collections::BTreeMap::new());
        self.crdt.merge(ops);
        self.build_result(None)
    }

    /// Compute a FractionalIndex position between two neighbor feature IDs.
    ///
    /// Pass `before_id_json` and `after_id_json` as feature ID strings (or empty/"" for boundaries).
    /// Returns the FractionalIndex as a JSON string.
    pub fn compute_position_between(
        &self,
        before_id_json: &str,
        after_id_json: &str,
    ) -> String {
        let ordered = self.crdt.ordered_features();

        let before_pos = if before_id_json.is_empty() {
            None
        } else {
            parse_feature_id(before_id_json)
                .and_then(|fid| ordered.iter().find(|(id, _)| *id == fid))
                .map(|(_, f)| &f.position)
        };

        let after_pos = if after_id_json.is_empty() {
            None
        } else {
            parse_feature_id(after_id_json)
                .and_then(|fid| ordered.iter().find(|(id, _)| *id == fid))
                .map(|(_, f)| &f.position)
        };

        let pos = FractionalIndex::between(before_pos, after_pos);
        serde_json::to_string(&pos).unwrap_or_else(|_| "[]".to_string())
    }

    // -- Sync (for future collaboration) --

    /// Merge remote operations (JSON array of Op).
    pub fn merge_remote(&mut self, ops_json: &str) -> JsValue {
        if let Ok(ops) = serde_json::from_str(ops_json) {
            self.crdt.merge(ops);
        }
        self.build_result(None)
    }

    /// Get the sync clock as JSON.
    pub fn get_sync_clock(&self) -> String {
        let clock: HashMap<String, u64> = self
            .crdt
            .clock()
            .iter()
            .map(|(k, v)| (k.0.to_string(), *v))
            .collect();
        serde_json::to_string(&clock).unwrap_or_else(|_| "{}".to_string())
    }

    /// Get operations since a remote clock state (JSON).
    pub fn get_ops_since(&self, remote_clock_json: &str) -> String {
        let remote_clock = if let Ok(map) =
            serde_json::from_str::<HashMap<String, u64>>(remote_clock_json)
        {
            map.into_iter()
                .filter_map(|(k, v)| k.parse::<u64>().ok().map(|r| (ReplicaId(r), v)))
                .collect()
        } else {
            std::collections::BTreeMap::new()
        };
        let ops = self.crdt.ops_since(&remote_clock);
        serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string())
    }
}

impl WasmDocumentEngine {
    /// Parse legacy v1 bytes and migrate to CRDT.
    fn from_v1_bytes(bytes: &[u8]) -> Result<WasmDocumentEngine, JsError> {
        let doc: vcad_ir::Document =
            serde_json::from_slice(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        let crdt = migrate_v1(&doc);
        Ok(Self { crdt })
    }

    /// Build the standard mutation result: { document, parts, createdFeatureId? }
    fn build_result(&self, created_fid: Option<FeatureId>) -> JsValue {
        let result = materialize(&self.crdt);
        let doc_json = serde_json::to_string(&result.document).unwrap_or_default();
        let parts_json = serde_json::to_string(&result.parts).unwrap_or_default();

        let mut obj = serde_json::json!({
            "document": serde_json::from_str::<serde_json::Value>(&doc_json).unwrap_or_default(),
            "parts": serde_json::from_str::<serde_json::Value>(&parts_json).unwrap_or_default(),
        });
        if let Some(fid) = created_fid {
            obj["createdFeatureId"] =
                serde_json::Value::String(format!("{}:{}", fid.0 .0, fid.1));
        }

        serde_wasm_bindgen::to_value(&obj).unwrap_or(JsValue::NULL)
    }
}

impl Default for WasmDocumentEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a feature ID from a JSON string like "\"1:0\"" or just "1:0".
fn parse_feature_id(json: &str) -> Option<FeatureId> {
    let s = json.trim_matches('"');
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let replica = parts[0].parse::<u64>().ok()?;
        let seq = parts[1].parse::<u64>().ok()?;
        Some(FeatureId(ReplicaId(replica), seq))
    } else {
        None
    }
}
