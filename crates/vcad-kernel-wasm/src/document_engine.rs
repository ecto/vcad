//! WASM bindings for the CRDT document engine.
//!
//! Exposes `WasmDocumentEngine` for browser/WASM consumers. Each mutation
//! returns a JSON object with `{ document, parts, consumedPartIds, createdFeatureId? }`.
//!
//! Internally uses `DocumentApi` for typed mutations. Legacy low-level CRDT
//! methods (`create_feature`, `set_param`) are kept for backward compatibility.

use std::collections::HashMap;

use serde::Serialize;
use vcad_app::document_api::{ApiResult, DocumentApi};
use vcad_app::feature::FeatureInput;
use vcad_app::materializer::materialize;
use vcad_app::migrate::{detect_format, migrate_v1, FileFormat};
use vcad_crdt::{CrdtDocument, FeatureId, FractionalIndex, ReplicaId, Value};
use wasm_bindgen::prelude::*;

/// CRDT-backed document engine for WASM.
///
/// Wraps a `DocumentApi` (which wraps a `CrdtDocument`) and exposes both
/// typed mutations via `add_feature(json)` and legacy low-level CRDT methods.
#[wasm_bindgen]
pub struct WasmDocumentEngine {
    api: DocumentApi,
}

#[wasm_bindgen]
impl WasmDocumentEngine {
    /// Create a new empty document engine.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        let replica_id = ReplicaId(js_sys::Date::now() as u64);
        Self {
            api: DocumentApi::new(replica_id),
        }
    }

    // -- Typed API (new) --

    /// Add a feature from a JSON-serialized `FeatureInput` discriminated union.
    ///
    /// Example: `{"type":"Cube","size_x":10,"size_y":20,"size_z":30}`
    ///
    /// Returns `{ document, parts, consumedPartIds, createdFeatureId }`.
    pub fn add_feature(&mut self, input_json: &str) -> JsValue {
        let input: FeatureInput = match serde_json::from_str(input_json) {
            Ok(i) => i,
            Err(_) => return self.serialize_api_result(&self.api.get_state()),
        };
        let result = self.api.add_feature(input);
        self.serialize_api_result(&result)
    }

    /// Update a feature with new params from a JSON-serialized `FeatureInput`.
    pub fn update_feature(&mut self, stable_id: &str, input_json: &str) -> JsValue {
        let input: FeatureInput = match serde_json::from_str(input_json) {
            Ok(i) => i,
            Err(_) => return self.serialize_api_result(&self.api.get_state()),
        };
        let result = self.api.update_feature(stable_id, input);
        self.serialize_api_result(&result)
    }

    /// Delete a feature by stable ID.
    pub fn delete_feature_by_id(&mut self, stable_id: &str) -> JsValue {
        let result = self.api.delete_feature(stable_id);
        self.serialize_api_result(&result)
    }

    /// Set translation on a feature.
    pub fn set_translation(&mut self, stable_id: &str, x: f64, y: f64, z: f64) -> JsValue {
        let result = self.api.set_translation(stable_id, [x, y, z]);
        self.serialize_api_result(&result)
    }

    /// Set rotation on a feature.
    pub fn set_rotation(&mut self, stable_id: &str, x: f64, y: f64, z: f64) -> JsValue {
        let result = self.api.set_rotation(stable_id, [x, y, z]);
        self.serialize_api_result(&result)
    }

    /// Set scale on a feature.
    pub fn set_scale(&mut self, stable_id: &str, x: f64, y: f64, z: f64) -> JsValue {
        let result = self.api.set_scale(stable_id, [x, y, z]);
        self.serialize_api_result(&result)
    }

    /// Set material on a feature.
    pub fn set_material(&mut self, stable_id: &str, material: &str) -> JsValue {
        let result = self.api.set_material(stable_id, material);
        self.serialize_api_result(&result)
    }

    /// Set visibility on a feature.
    pub fn set_visible(&mut self, stable_id: &str, visible: bool) -> JsValue {
        let result = self.api.set_visible(stable_id, visible);
        self.serialize_api_result(&result)
    }

    /// Rename a feature.
    pub fn rename_feature(&mut self, stable_id: &str, name: &str) -> JsValue {
        let result = self.api.rename_feature(stable_id, name);
        self.serialize_api_result(&result)
    }

    /// Set joint state.
    pub fn set_joint_state(&mut self, stable_id: &str, state: f64) -> JsValue {
        let result = self.api.set_joint_state(stable_id, state);
        self.serialize_api_result(&result)
    }

    // -- Legacy low-level CRDT methods (backward compatibility) --

    /// Create a feature with the given kind and params (JSON string).
    ///
    /// Returns `{ document, parts, createdFeatureId }` as a JsValue.
    pub fn create_feature(&mut self, kind: &str, params_json: &str) -> JsValue {
        let params: HashMap<String, Value> = serde_json::from_str(params_json).unwrap_or_default();
        // Route through DocumentApi so the new feature gets a registered stable
        // id and is therefore movable / updatable (e.g. a PCB board).
        let result = self.api.create_feature_raw(kind, params);
        self.serialize_api_result(&result)
    }

    /// Delete a feature by ID (JSON string).
    pub fn delete_feature(&mut self, feature_id_json: &str) -> JsValue {
        if let Some(fid) = parse_feature_id(feature_id_json) {
            self.api.crdt_mut().delete_feature(fid);
        }
        self.build_result(None)
    }

    /// Set a parameter on a feature.
    pub fn set_param(&mut self, feature_id_json: &str, key: &str, value_json: &str) -> JsValue {
        if let (Some(fid), Ok(value)) = (
            parse_feature_id(feature_id_json),
            serde_json::from_str::<Value>(value_json),
        ) {
            self.api.crdt_mut().set_param(fid, key, value);
        }
        self.build_result(None)
    }

    /// Move a feature to a new position.
    pub fn move_feature(&mut self, feature_id_json: &str, position_json: &str) -> JsValue {
        if let (Some(fid), Ok(pos)) = (
            parse_feature_id(feature_id_json),
            serde_json::from_str::<FractionalIndex>(position_json),
        ) {
            self.api.crdt_mut().move_feature(fid, pos);
        }
        self.build_result(None)
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> JsValue {
        self.api.crdt_mut().undo();
        self.build_result(None)
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) -> JsValue {
        self.api.crdt_mut().redo();
        self.build_result(None)
    }

    /// Whether undo is available.
    pub fn can_undo(&self) -> bool {
        self.api.crdt().can_undo()
    }

    /// Whether redo is available.
    pub fn can_redo(&self) -> bool {
        self.api.crdt().can_redo()
    }

    /// Get the materialized document as JSON.
    pub fn get_document_json(&self) -> String {
        let result = materialize(self.api.crdt());
        serde_json::to_string(&result.document).unwrap_or_else(|_| "{}".to_string())
    }

    /// Get the parts list as JSON.
    pub fn get_parts_json(&self) -> String {
        let result = materialize(self.api.crdt());
        serde_json::to_string(&result.parts).unwrap_or_else(|_| "[]".to_string())
    }

    /// Get ordered features (for the feature tree) as JSON.
    pub fn get_ordered_features_json(&self) -> String {
        let features: Vec<_> = self
            .api
            .crdt()
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
        self.api.save()
    }

    /// Load a document from bytes.
    ///
    /// Auto-detects format: if CRDT (v2), loads directly; if legacy JSON (v1),
    /// migrates to CRDT first.
    pub fn load(bytes: &[u8]) -> Result<WasmDocumentEngine, JsError> {
        match detect_format(bytes) {
            FileFormat::V2Crdt => {
                let crdt = CrdtDocument::load(bytes).map_err(|e| JsError::new(&e.to_string()))?;
                Ok(Self::from_crdt(crdt))
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
    pub fn import_ir(&mut self, ir_json: &str) -> JsValue {
        let doc: vcad_ir::Document = match serde_json::from_str(ir_json) {
            Ok(d) => d,
            Err(_) => return self.build_result(None),
        };
        let migrated = migrate_v1(&doc);
        let ops = migrated.ops_since(&std::collections::BTreeMap::new());
        self.api.crdt_mut().merge(ops);
        self.build_result(None)
    }

    /// Compute a FractionalIndex position between two neighbor feature IDs.
    pub fn compute_position_between(&self, before_id_json: &str, after_id_json: &str) -> String {
        let ordered = self.api.crdt().ordered_features();

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

    // -- Sync --

    /// Merge remote operations (JSON array of Op).
    pub fn merge_remote(&mut self, ops_json: &str) -> JsValue {
        if let Ok(ops) = serde_json::from_str(ops_json) {
            self.api.crdt_mut().merge(ops);
        }
        self.build_result(None)
    }

    /// Get the sync clock as JSON.
    pub fn get_sync_clock(&self) -> String {
        let clock: HashMap<String, u64> = self
            .api
            .crdt()
            .clock()
            .iter()
            .map(|(k, v)| (k.0.to_string(), *v))
            .collect();
        serde_json::to_string(&clock).unwrap_or_else(|_| "{}".to_string())
    }

    /// Get operations since a remote clock state (JSON).
    pub fn get_ops_since(&self, remote_clock_json: &str) -> String {
        let remote_clock =
            if let Ok(map) = serde_json::from_str::<HashMap<String, u64>>(remote_clock_json) {
                map.into_iter()
                    .filter_map(|(k, v)| k.parse::<u64>().ok().map(|r| (ReplicaId(r), v)))
                    .collect()
            } else {
                std::collections::BTreeMap::new()
            };
        let ops = self.api.crdt().ops_since(&remote_clock);
        serde_json::to_string(&ops).unwrap_or_else(|_| "[]".to_string())
    }
}

impl WasmDocumentEngine {
    /// Create from an existing CrdtDocument.
    fn from_crdt(crdt: CrdtDocument) -> Self {
        let replica_id = ReplicaId(js_sys::Date::now() as u64);
        let mut api = DocumentApi::new(replica_id);
        // Replace the fresh CRDT with the loaded one.
        *api.crdt_mut() = crdt;
        // Rebuild the stable-id map so existing features can be resolved
        // by their stable IDs (delete, rename, set_translation, etc.).
        api.rebuild_stable_ids();
        Self { api }
    }

    /// Parse legacy v1 bytes and migrate to CRDT.
    fn from_v1_bytes(bytes: &[u8]) -> Result<WasmDocumentEngine, JsError> {
        let doc: vcad_ir::Document =
            serde_json::from_slice(bytes).map_err(|e| JsError::new(&e.to_string()))?;
        let crdt = migrate_v1(&doc);
        Ok(Self::from_crdt(crdt))
    }

    /// Build the legacy mutation result: `{ document, parts, createdFeatureId? }`
    ///
    /// Used by backward-compatible methods. New typed methods use `serialize_api_result`.
    fn build_result(&self, created_fid: Option<FeatureId>) -> JsValue {
        let result = materialize(self.api.crdt());
        let doc_json = serde_json::to_string(&result.document).unwrap_or_default();
        let parts_json = serde_json::to_string(&result.parts).unwrap_or_default();

        let mut obj = serde_json::json!({
            "document": serde_json::from_str::<serde_json::Value>(&doc_json).unwrap_or_default(),
            "parts": serde_json::from_str::<serde_json::Value>(&parts_json).unwrap_or_default(),
        });
        if let Some(fid) = created_fid {
            obj["createdFeatureId"] = serde_json::Value::String(format!("{}:{}", fid.0 .0, fid.1));
        }

        let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
        obj.serialize(&serializer).unwrap_or(JsValue::NULL)
    }

    /// Serialize an ApiResult to a JsValue.
    ///
    /// Returns `{ document, parts, consumedPartIds, createdFeatureId? }`.
    fn serialize_api_result(&self, result: &ApiResult) -> JsValue {
        let doc_json = serde_json::to_string(&result.document).unwrap_or_default();
        let parts_json = serde_json::to_string(&result.parts).unwrap_or_default();

        // Surface materialization warnings as console.warn so users notice
        // when a feature has been skipped because its input ref doesn't
        // resolve. Non-fatal — they're also returned in the ApiResult.
        for w in &result.warnings {
            web_sys::console::warn_1(&format!("[vcad] {w}").into());
        }

        let mut obj = serde_json::json!({
            "document": serde_json::from_str::<serde_json::Value>(&doc_json).unwrap_or_default(),
            "parts": serde_json::from_str::<serde_json::Value>(&parts_json).unwrap_or_default(),
            "consumedPartIds": result.consumed_part_ids,
            "warnings": result.warnings,
        });
        if let Some(id) = &result.created_feature_id {
            obj["createdFeatureId"] = serde_json::Value::String(id.clone());
        }

        let serializer = serde_wasm_bindgen::Serializer::new().serialize_maps_as_objects(true);
        obj.serialize(&serializer).unwrap_or(JsValue::NULL)
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
