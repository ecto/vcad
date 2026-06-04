//! High-level document API — typed mutations over the CRDT.
//!
//! `DocumentApi` wraps `CrdtDocument` and provides typed methods that accept
//! `FeatureInput` values. It manages stable ID mapping and materializes after
//! each mutation.

use std::collections::HashMap;

use vcad_crdt::{CrdtDocument, FeatureId, FractionalIndex, ReplicaId, Value};
use vcad_ir::Document;

use crate::feature::FeatureInput;
use crate::materializer::{materialize, MaterializeResult};
use crate::part_info::PartInfo;

/// Bidirectional mapping between stable string IDs and CRDT FeatureIds.
///
/// Stable IDs are formatted as `"replica:seq"` (e.g. `"1738000000:0"`).
/// This struct is intentionally simple — it's just the string formatting
/// that the rest of the system already uses.
#[derive(Debug, Clone, Default)]
pub struct StableIdMap {
    /// FeatureId → stable string.
    fid_to_stable: HashMap<FeatureId, String>,
    /// Stable string → FeatureId.
    stable_to_fid: HashMap<String, FeatureId>,
}

impl StableIdMap {
    /// Create an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a mapping.
    pub fn insert(&mut self, fid: FeatureId) -> String {
        let stable = format!("{}:{}", fid.0 .0, fid.1);
        self.fid_to_stable.insert(fid, stable.clone());
        self.stable_to_fid.insert(stable.clone(), fid);
        stable
    }

    /// Look up a FeatureId by its stable string.
    pub fn resolve(&self, stable_id: &str) -> Option<FeatureId> {
        self.stable_to_fid.get(stable_id).copied()
    }

    /// Look up a stable string by FeatureId.
    pub fn stable_id(&self, fid: FeatureId) -> Option<&str> {
        self.fid_to_stable.get(&fid).map(|s| s.as_str())
    }

    /// Rebuild the map from the CRDT's current features.
    pub fn rebuild(&mut self, crdt: &CrdtDocument) {
        self.fid_to_stable.clear();
        self.stable_to_fid.clear();
        for (fid, _) in crdt.ordered_features() {
            self.insert(fid);
        }
        // Also include deleted features that have been seen
        // (they may still be referenced by other features).
    }
}

/// Result returned by every `DocumentApi` mutation.
#[derive(Debug, Clone)]
pub struct ApiResult {
    /// The materialized IR document.
    pub document: Document,
    /// Part metadata for each materialized feature.
    pub parts: Vec<PartInfo>,
    /// Stable IDs of parts consumed by other features (e.g. boolean inputs).
    pub consumed_part_ids: Vec<String>,
    /// Stable ID of the newly created feature, if any.
    pub created_feature_id: Option<String>,
    /// Non-fatal issues detected during materialization (e.g. dangling
    /// input references). Consumers should surface these to the user.
    pub warnings: Vec<String>,
}

impl ApiResult {
    /// Build from a MaterializeResult, computing consumed parts.
    fn from_materialize(result: MaterializeResult, created_id: Option<String>) -> Self {
        let consumed = compute_consumed_part_ids(&result.parts);
        Self {
            document: result.document,
            parts: result.parts,
            consumed_part_ids: consumed,
            created_feature_id: created_id,
            warnings: result.warnings,
        }
    }
}

/// High-level document API.
///
/// Wraps a `CrdtDocument` with typed mutation methods. Each mutation
/// materializes the document and returns the full state.
pub struct DocumentApi {
    crdt: CrdtDocument,
    stable_ids: StableIdMap,
    name: String,
    dirty: bool,
}

impl DocumentApi {
    /// Create a new empty document.
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            crdt: CrdtDocument::new(replica_id),
            stable_ids: StableIdMap::new(),
            name: "Untitled".into(),
            dirty: false,
        }
    }

    /// Add a feature with typed input.
    pub fn add_feature(&mut self, input: FeatureInput) -> ApiResult {
        let (kind, params) = input.to_crdt_params();

        let ordered = self.crdt.ordered_features();
        let position = if let Some(last) = ordered.last() {
            FractionalIndex::between(Some(&last.1.position), None)
        } else {
            FractionalIndex::between(None, None)
        };

        let (fid, _cs) = self.crdt.create_feature(kind, position, params);
        let stable_id = self.stable_ids.insert(fid);
        self.dirty = true;
        self.build_result(Some(stable_id))
    }

    /// Create a feature from a raw kind + params map, for features that have no
    /// typed [`FeatureInput`] constructor (e.g. `pcb-board`, which carries a
    /// large serialized `board` param). Mirrors [`add_feature`](Self::add_feature)
    /// — crucially it registers a stable id, so the feature can later be
    /// transformed (`set_translation`/`set_rotation`/`set_scale`), updated, or
    /// deleted by id. Without this a board would be unmovable.
    pub fn create_feature_raw(&mut self, kind: &str, params: HashMap<String, Value>) -> ApiResult {
        let ordered = self.crdt.ordered_features();
        let position = if let Some(last) = ordered.last() {
            FractionalIndex::between(Some(&last.1.position), None)
        } else {
            FractionalIndex::between(None, None)
        };

        let (fid, _cs) = self.crdt.create_feature(kind, position, params);
        let stable_id = self.stable_ids.insert(fid);
        self.dirty = true;
        self.build_result(Some(stable_id))
    }

    /// Update all params on an existing feature by replacing with new input.
    pub fn update_feature(&mut self, stable_id: &str, input: FeatureInput) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            let (_kind, params) = input.to_crdt_params();
            for (key, value) in params {
                self.crdt.set_param(fid, &key, value);
            }
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Delete a feature.
    pub fn delete_feature(&mut self, stable_id: &str) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.delete_feature(fid);
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Set translation offset.
    pub fn set_translation(&mut self, stable_id: &str, offset: [f64; 3]) -> ApiResult {
        self.set_vec3_param(stable_id, "offset", offset)
    }

    /// Set rotation angles.
    pub fn set_rotation(&mut self, stable_id: &str, angles: [f64; 3]) -> ApiResult {
        self.set_vec3_param(stable_id, "rotation", angles)
    }

    /// Set scale factor.
    pub fn set_scale(&mut self, stable_id: &str, factor: [f64; 3]) -> ApiResult {
        self.set_vec3_param(stable_id, "scale", factor)
    }

    /// Set material.
    pub fn set_material(&mut self, stable_id: &str, material: &str) -> ApiResult {
        self.set_str_param(stable_id, "material", material)
    }

    /// Set visibility.
    pub fn set_visible(&mut self, stable_id: &str, visible: bool) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.set_param(fid, "visible", Value::Bool(visible));
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Rename a feature.
    pub fn rename_feature(&mut self, stable_id: &str, name: &str) -> ApiResult {
        self.set_str_param(stable_id, "name", name)
    }

    /// Reorder a feature to a new position.
    pub fn reorder_feature(&mut self, stable_id: &str, new_index: u32) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            let ordered = self.crdt.ordered_features();
            let idx = new_index as usize;

            let before = if idx > 0 {
                ordered.get(idx - 1).map(|(_, f)| &f.position)
            } else {
                None
            };
            let after = ordered.get(idx).map(|(_, f)| &f.position);

            let pos = FractionalIndex::between(before, after);
            self.crdt.move_feature(fid, pos);
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Duplicate features.
    pub fn duplicate_features(&mut self, stable_ids: &[String]) -> ApiResult {
        let mut last_created = None;

        for sid in stable_ids {
            if let Some(fid) = self.stable_ids.resolve(sid) {
                // Read the feature's kind and params
                let ordered = self.crdt.ordered_features();
                if let Some((_, feature)) = ordered.iter().find(|(id, _)| *id == fid) {
                    let kind = feature.kind.clone();
                    let params: HashMap<String, Value> = feature
                        .params
                        .iter()
                        .map(|(k, (v, _))| (k.clone(), v.clone()))
                        .collect();

                    // Insert at end
                    let ordered = self.crdt.ordered_features();
                    let position = if let Some(last) = ordered.last() {
                        FractionalIndex::between(Some(&last.1.position), None)
                    } else {
                        FractionalIndex::between(None, None)
                    };

                    let (new_fid, _) = self.crdt.create_feature(&kind, position, params);
                    last_created = Some(self.stable_ids.insert(new_fid));
                }
            }
        }
        self.dirty = true;
        self.build_result(last_created)
    }

    /// Set joint state value.
    pub fn set_joint_state(&mut self, stable_id: &str, state: f64) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.set_param(fid, "state", Value::F64(state));
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> ApiResult {
        self.crdt.undo();
        self.dirty = true;
        self.build_result(None)
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) -> ApiResult {
        self.crdt.redo();
        self.dirty = true;
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

    /// Save the document to bytes.
    pub fn save(&self) -> Vec<u8> {
        self.crdt.save()
    }

    /// Load a document from bytes.
    pub fn load(bytes: &[u8], replica_id: ReplicaId) -> Result<Self, String> {
        let crdt = CrdtDocument::load(bytes).map_err(|e| e.to_string())?;
        let mut api = Self {
            crdt,
            stable_ids: StableIdMap::new(),
            name: "Untitled".into(),
            dirty: false,
        };
        let _ = replica_id;
        api.stable_ids.rebuild(&api.crdt);
        Ok(api)
    }

    /// Import IR JSON into the document.
    pub fn import_ir(&mut self, ir_json: &str) -> ApiResult {
        if let Ok(doc) = serde_json::from_str::<vcad_ir::Document>(ir_json) {
            let migrated = crate::migrate::migrate_v1(&doc);
            let ops = migrated.ops_since(&std::collections::BTreeMap::new());
            self.crdt.merge(ops);
            self.stable_ids.rebuild(&self.crdt);
            self.dirty = true;
        }
        self.build_result(None)
    }

    /// Get the current document state.
    pub fn get_state(&self) -> ApiResult {
        self.build_result(None)
    }

    /// Get a raw param value by key.
    pub fn set_param(&mut self, stable_id: &str, key: &str, value: Value) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.set_param(fid, key, value);
            self.dirty = true;
        }
        self.build_result(None)
    }

    // -- Metadata --

    /// Get the document name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Set the document name.
    pub fn set_name(&mut self, name: &str) {
        self.name = name.into();
    }

    /// Whether the document has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the document as saved.
    pub fn mark_saved(&mut self) {
        self.dirty = false;
    }

    /// Access the underlying CRDT document (for sync, ordered features, etc.).
    pub fn crdt(&self) -> &CrdtDocument {
        &self.crdt
    }

    /// Mutable access to the CRDT (for sync operations).
    pub fn crdt_mut(&mut self) -> &mut CrdtDocument {
        &mut self.crdt
    }

    /// Access the stable ID map.
    pub fn stable_ids(&self) -> &StableIdMap {
        &self.stable_ids
    }

    /// Rebuild the stable-id map from the current CRDT state.
    ///
    /// Must be called after loading or replacing the CRDT document so that
    /// existing features can be resolved by their stable IDs.
    pub fn rebuild_stable_ids(&mut self) {
        self.stable_ids.rebuild(&self.crdt);
    }

    // -- Internal helpers --

    fn set_vec3_param(&mut self, stable_id: &str, key: &str, value: [f64; 3]) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.set_param(fid, key, Value::Vec3(value));
            self.dirty = true;
        }
        self.build_result(None)
    }

    fn set_str_param(&mut self, stable_id: &str, key: &str, value: &str) -> ApiResult {
        if let Some(fid) = self.stable_ids.resolve(stable_id) {
            self.crdt.set_param(fid, key, Value::String(value.into()));
            self.dirty = true;
        }
        self.build_result(None)
    }

    fn build_result(&self, created_id: Option<String>) -> ApiResult {
        let result = materialize(&self.crdt);
        ApiResult::from_materialize(result, created_id)
    }
}

/// Compute which part IDs are consumed (referenced as inputs) by other parts.
///
/// A part is "consumed" if it's used as an input to a boolean, fillet, chamfer,
/// shell, pattern, or mirror operation.
fn compute_consumed_part_ids(parts: &[PartInfo]) -> Vec<String> {
    let mut consumed = Vec::new();
    for part in parts {
        match part {
            PartInfo::Boolean {
                source_part_ids, ..
            } => {
                consumed.extend(source_part_ids.iter().cloned());
            }
            PartInfo::Fillet { source_part_id, .. }
            | PartInfo::Chamfer { source_part_id, .. }
            | PartInfo::Shell { source_part_id, .. }
            | PartInfo::LinearPattern { source_part_id, .. }
            | PartInfo::CircularPattern { source_part_id, .. }
            | PartInfo::Mirror { source_part_id, .. } => {
                consumed.push(source_part_id.clone());
            }
            _ => {}
        }
    }
    consumed.sort();
    consumed.dedup();
    consumed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::BooleanType;

    fn test_api() -> DocumentApi {
        DocumentApi::new(ReplicaId(1))
    }

    #[test]
    fn test_add_cube() {
        let mut api = test_api();
        let result = api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 20.0,
            size_z: 30.0,
        });
        assert_eq!(result.parts.len(), 1);
        assert!(result.created_feature_id.is_some());
        assert!(api.is_dirty());
    }

    #[test]
    fn test_delete_feature() {
        let mut api = test_api();
        let result = api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 10.0,
            size_z: 10.0,
        });
        let id = result.created_feature_id.unwrap();

        let result = api.delete_feature(&id);
        assert_eq!(result.parts.len(), 0);
    }

    #[test]
    fn test_consumed_parts() {
        let mut api = test_api();

        let r1 = api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 10.0,
            size_z: 10.0,
        });
        let id1 = r1.created_feature_id.unwrap();

        let r2 = api.add_feature(FeatureInput::Cylinder {
            radius: 5.0,
            height: 10.0,
            segments: None,
        });
        let id2 = r2.created_feature_id.unwrap();

        let result = api.add_feature(FeatureInput::Boolean {
            boolean_type: BooleanType::Difference,
            input_a: id1.clone(),
            input_b: id2.clone(),
        });

        assert_eq!(result.consumed_part_ids.len(), 2);
        assert!(result.consumed_part_ids.contains(&id1));
        assert!(result.consumed_part_ids.contains(&id2));
    }

    #[test]
    fn test_undo_redo() {
        let mut api = test_api();
        assert!(!api.can_undo());

        api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 10.0,
            size_z: 10.0,
        });
        assert!(api.can_undo());

        let result = api.undo();
        assert_eq!(result.parts.len(), 0);
        assert!(api.can_redo());

        let result = api.redo();
        assert_eq!(result.parts.len(), 1);
    }

    #[test]
    fn test_set_translation() {
        let mut api = test_api();
        let r = api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 10.0,
            size_z: 10.0,
        });
        let id = r.created_feature_id.unwrap();

        let result = api.set_translation(&id, [5.0, 10.0, 15.0]);
        assert_eq!(result.parts.len(), 1);
        // Verify the translate node has the right offset
        let translate_id = result.parts[0].root_node_id();
        let node = result.document.nodes.get(&translate_id).unwrap();
        match &node.op {
            vcad_ir::CsgOp::Translate { offset, .. } => {
                assert_eq!(offset.x, 5.0);
                assert_eq!(offset.y, 10.0);
                assert_eq!(offset.z, 15.0);
            }
            _ => panic!("expected Translate"),
        }
    }

    #[test]
    fn create_feature_raw_is_movable() {
        // A feature created via the raw path (e.g. a PCB board) must register a
        // stable id so it can be transformed like any other part. Regression
        // for boards being unmovable.
        let mut api = test_api();
        let r = api.create_feature_raw("pcb-board", HashMap::new());
        let id = r
            .created_feature_id
            .expect("create_feature_raw should return a registered stable id");

        let result = api.set_translation(&id, [5.0, 10.0, 15.0]);
        assert_eq!(result.parts.len(), 1);
        let translate_id = result.parts[0].root_node_id();
        match &result.document.nodes.get(&translate_id).unwrap().op {
            vcad_ir::CsgOp::Translate { offset, .. } => {
                assert_eq!((offset.x, offset.y, offset.z), (5.0, 10.0, 15.0));
            }
            other => panic!("expected Translate, got {other:?}"),
        }
    }

    #[test]
    fn test_save_load_roundtrip() {
        let mut api = test_api();
        api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 20.0,
            size_z: 30.0,
        });

        let bytes = api.save();
        let api2 = DocumentApi::load(&bytes, ReplicaId(2)).unwrap();
        let state = api2.get_state();
        assert_eq!(state.parts.len(), 1);
    }

    #[test]
    fn test_pork_chop_fillet_chat_flow_produces_single_root() {
        // End-to-end regression through the high-level DocumentApi — exactly
        // the path the TS side exercises via `engine.add_feature(JSON)`. A
        // fillet's input-consumption logic must ensure only the fillet, not
        // its pre-fillet extrude source, ends up in `doc.roots`. Otherwise
        // both near-identical shells render and z-fight ("the pork-chop
        // sawtooth").
        let sketch_json = serde_json::to_string(&vcad_ir::CsgOp::Sketch2D {
            origin: vcad_ir::Vec3::new(0.0, 0.0, 0.0),
            x_dir: vcad_ir::Vec3::new(1.0, 0.0, 0.0),
            y_dir: vcad_ir::Vec3::new(0.0, 1.0, 0.0),
            segments: vec![
                vcad_ir::SketchSegment2D::Line {
                    start: vcad_ir::Vec2 { x: 0.0, y: 0.0 },
                    end: vcad_ir::Vec2 { x: 10.0, y: 0.0 },
                },
                vcad_ir::SketchSegment2D::Line {
                    start: vcad_ir::Vec2 { x: 10.0, y: 0.0 },
                    end: vcad_ir::Vec2 { x: 10.0, y: 10.0 },
                },
                vcad_ir::SketchSegment2D::Line {
                    start: vcad_ir::Vec2 { x: 10.0, y: 10.0 },
                    end: vcad_ir::Vec2 { x: 0.0, y: 10.0 },
                },
                vcad_ir::SketchSegment2D::Line {
                    start: vcad_ir::Vec2 { x: 0.0, y: 10.0 },
                    end: vcad_ir::Vec2 { x: 0.0, y: 0.0 },
                },
            ],
        })
        .unwrap();

        let mut api = test_api();

        // 1. Extrude the sketch ("Chop Meat").
        let r1 = api.add_feature(FeatureInput::Extrude {
            sketch: sketch_json.clone(),
            depth: 20.0,
            direction: [0.0, 0.0, 1.0],
            twist_angle: None,
            scale_end: None,
        });
        let extrude_id = r1.created_feature_id.clone().unwrap();

        // 2. Fillet that extrude ("Meat Rounding") — chat path passes the
        //    stable id of the extrude as `input`.
        let r2 = api.add_feature(FeatureInput::Fillet {
            input: extrude_id.clone(),
            radius: 5.0,
        });

        // Extrude is reported as consumed.
        assert!(
            r2.consumed_part_ids.contains(&extrude_id),
            "fillet must report its source as consumed; got consumed={:?}",
            r2.consumed_part_ids
        );

        // 3. Set material on the pre-fillet extrude (as the AI did).
        let r3 = api.set_material(&extrude_id, "abs-red");

        // The document after all of this must have exactly ONE scene root —
        // the fillet. The pre-fillet extrude is consumed and not rooted.
        assert_eq!(
            r3.document.roots.len(),
            1,
            "expected exactly 1 root (fillet); got {} roots: {:#?}",
            r3.document.roots.len(),
            r3.document.roots
        );
    }

    #[test]
    fn test_update_feature() {
        let mut api = test_api();
        let r = api.add_feature(FeatureInput::Cube {
            size_x: 10.0,
            size_y: 10.0,
            size_z: 10.0,
        });
        let id = r.created_feature_id.unwrap();

        let result = api.update_feature(
            &id,
            FeatureInput::Cube {
                size_x: 20.0,
                size_y: 30.0,
                size_z: 40.0,
            },
        );
        assert_eq!(result.parts.len(), 1);

        // Verify the prim node uses new dimensions
        let prim_id = match &result.parts[0] {
            PartInfo::Cube {
                primitive_node_id, ..
            } => *primitive_node_id,
            _ => panic!("expected cube"),
        };
        match &result.document.nodes.get(&prim_id).unwrap().op {
            vcad_ir::CsgOp::Cube { size } => {
                assert_eq!(size.x, 20.0);
                assert_eq!(size.y, 30.0);
                assert_eq!(size.z, 40.0);
            }
            _ => panic!("expected Cube op"),
        }
    }
}
