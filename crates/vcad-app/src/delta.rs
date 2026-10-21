//! Command deltas: what a command changed, instead of the whole
//! materialized document.
//!
//! [`DocumentApi::apply_json_delta_envelope`] answers a command with an
//! [`EngineDeltaEnvelope`]: a [`DocumentDelta`] patching the previous IR
//! document plus a [`PartsDelta`] naming the parts (by stable id) that were
//! added, updated or removed. Hosts keep the previous document and apply the
//! patch ([`DocumentDelta::apply`]); applying it always reproduces the full
//! materialize exactly (see `tests/command_algebra.rs`). Loads and imports
//! keep using the full-document envelope.
//!
//! Node ids are allocated in per-feature blocks (see the materializer), so a
//! command touching one part leaves every other part's nodes byte-identical
//! and the patch stays proportional to the edit.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::command::{CommandError, EngineError};
use crate::document_api::{ApiResult, DocumentApi};
use crate::part_info::PartInfo;

/// Per-key changes to one object-valued document field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct MapPatch {
    /// Keys added or changed, with their new values.
    #[serde(default)]
    #[cfg_attr(feature = "ts-rs", ts(type = "Record<string, unknown>"))]
    pub set: BTreeMap<String, Value>,
    /// Keys removed.
    #[serde(default)]
    pub remove: Vec<String>,
}

/// A patch from one materialized IR document to the next.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub struct DocumentDelta {
    /// Per-key patches for object-valued top-level fields that are objects
    /// on both sides: `nodes`, `materials`, `parameters`, `partIds`, ….
    #[serde(default)]
    #[cfg_attr(feature = "ts-rs", ts(type = "Record<string, MapPatch>"))]
    pub maps: BTreeMap<String, MapPatch>,
    /// Other top-level fields that changed, replaced wholesale (`roots`, …).
    #[serde(default)]
    #[cfg_attr(feature = "ts-rs", ts(type = "Record<string, unknown>"))]
    pub fields: BTreeMap<String, Value>,
    /// Top-level fields that disappeared.
    #[serde(default)]
    pub removed_fields: Vec<String>,
}

impl DocumentDelta {
    /// The patch taking `prev` to `next` (both IR documents as JSON objects).
    pub fn between(prev: &Value, next: &Value) -> Self {
        let empty = serde_json::Map::new();
        let prev = prev.as_object().unwrap_or(&empty);
        let next = next.as_object().unwrap_or(&empty);
        let mut delta = Self::default();
        for (key, new) in next {
            match (prev.get(key), new) {
                (Some(old), _) if old == new => {}
                (Some(Value::Object(old)), Value::Object(new)) => {
                    let patch = MapPatch {
                        set: new
                            .iter()
                            .filter(|(k, v)| old.get(*k) != Some(v))
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect(),
                        remove: old
                            .keys()
                            .filter(|k| !new.contains_key(*k))
                            .cloned()
                            .collect(),
                    };
                    delta.maps.insert(key.clone(), patch);
                }
                _ => {
                    delta.fields.insert(key.clone(), new.clone());
                }
            }
        }
        delta.removed_fields = prev
            .keys()
            .filter(|k| !next.contains_key(*k))
            .cloned()
            .collect();
        delta
    }

    /// Patch `doc` (the document this delta was computed from) in place.
    pub fn apply(&self, doc: &mut Value) {
        if !doc.is_object() {
            *doc = Value::Object(Default::default());
        }
        let obj = doc.as_object_mut().expect("object");
        for key in &self.removed_fields {
            obj.remove(key);
        }
        for (key, value) in &self.fields {
            obj.insert(key.clone(), value.clone());
        }
        for (key, patch) in &self.maps {
            let entry = obj
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Default::default()));
            if !entry.is_object() {
                *entry = Value::Object(Default::default());
            }
            let map = entry.as_object_mut().expect("object");
            for k in &patch.remove {
                map.remove(k);
            }
            for (k, v) in &patch.set {
                map.insert(k.clone(), v.clone());
            }
        }
    }

    /// Whether the patch changes nothing.
    pub fn is_empty(&self) -> bool {
        self.maps.is_empty() && self.fields.is_empty() && self.removed_fields.is_empty()
    }

    /// Ids of IR nodes the patch adds, changes or removes.
    fn touched_nodes(&self) -> HashSet<u64> {
        self.maps
            .get("nodes")
            .map(|p| {
                p.set
                    .keys()
                    .chain(&p.remove)
                    .filter_map(|k| k.parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// Which parts a command added, updated or removed, by stable id.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct PartsDelta {
    /// New parts.
    #[serde(default)]
    pub added: Vec<PartInfo>,
    /// Parts whose info or geometry (any node in their subtree) changed.
    /// Hosts re-tessellate exactly these plus `added`.
    #[serde(default)]
    pub updated: Vec<PartInfo>,
    /// Stable ids of parts that no longer exist.
    #[serde(default)]
    pub removed: Vec<String>,
    /// The full part order (stable ids), present when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub order: Option<Vec<String>>,
}

impl PartsDelta {
    /// Compare part lists, counting a part as updated when its info changed
    /// or any node reachable from its root is in `touched`.
    fn between(
        prev: &[PartInfo],
        next: &[PartInfo],
        doc: &vcad_ir::Document,
        touched: &HashSet<u64>,
    ) -> Self {
        let old: HashMap<&str, Value> = prev
            .iter()
            .map(|p| (p.id(), serde_json::to_value(p).unwrap_or_default()))
            .collect();
        let new_ids: HashSet<&str> = next.iter().map(PartInfo::id).collect();
        let mut delta = Self::default();
        for part in next {
            match old.get(part.id()) {
                None => delta.added.push(part.clone()),
                Some(before) => {
                    let info_changed = *before != serde_json::to_value(part).unwrap_or_default();
                    if info_changed || subtree_touches(doc, part.root_node_id(), touched) {
                        delta.updated.push(part.clone());
                    }
                }
            }
        }
        delta.removed = prev
            .iter()
            .map(PartInfo::id)
            .filter(|id| !new_ids.contains(id))
            .map(str::to_string)
            .collect();
        let order: Vec<&str> = next.iter().map(PartInfo::id).collect();
        if order != prev.iter().map(PartInfo::id).collect::<Vec<_>>() {
            delta.order = Some(order.into_iter().map(str::to_string).collect());
        }
        delta
    }

    /// Patch a part list (the one this delta was computed from) in place.
    pub fn apply(&self, parts: &mut Vec<PartInfo>) {
        // No `order` means the order didn't change (additions and removals
        // always change it).
        let order: Vec<String> = match &self.order {
            Some(o) => o.clone(),
            None => parts.iter().map(|p| p.id().to_string()).collect(),
        };
        let mut by_id: HashMap<String, PartInfo> =
            parts.drain(..).map(|p| (p.id().to_string(), p)).collect();
        for p in self.added.iter().chain(&self.updated) {
            by_id.insert(p.id().to_string(), p.clone());
        }
        for id in &self.removed {
            by_id.remove(id);
        }
        *parts = order.iter().filter_map(|id| by_id.remove(id)).collect();
    }

    /// Whether no part changed.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.removed.is_empty()
            && self.order.is_none()
    }
}

fn subtree_touches(doc: &vcad_ir::Document, root: u64, touched: &HashSet<u64>) -> bool {
    if touched.is_empty() {
        return false;
    }
    let mut stack = vec![root];
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if touched.contains(&id) {
            return true;
        }
        if let Some(node) = doc.nodes.get(&id) {
            stack.extend(node.op.child_ids());
        }
    }
    false
}

/// Successful delta result inside an [`EngineDeltaEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub struct EngineDelta {
    /// Patch for the previous IR document.
    pub document: DocumentDelta,
    /// Parts added, updated and removed.
    pub parts: PartsDelta,
    /// Stable ids of parts consumed by other features (full list; small).
    pub consumed_part_ids: Vec<String>,
    /// Non-fatal materialize issues.
    pub warnings: Vec<String>,
    /// Stable id of the feature the command created, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub created_feature_id: Option<String>,
}

impl EngineDelta {
    /// The delta from `prev` to `next`.
    pub fn between(prev: &ApiResult, next: ApiResult) -> Self {
        let before = serde_json::to_value(&prev.document).unwrap_or_default();
        let after = serde_json::to_value(&next.document).unwrap_or_default();
        let document = DocumentDelta::between(&before, &after);
        let parts = PartsDelta::between(
            &prev.parts,
            &next.parts,
            &next.document,
            &document.touched_nodes(),
        );
        Self {
            document,
            parts,
            consumed_part_ids: next.consumed_part_ids,
            warnings: next.warnings,
            created_feature_id: next.created_feature_id,
        }
    }
}

/// The delta counterpart of [`crate::command::EngineEnvelope`]:
/// `{"ok":true,"result":EngineDelta}` or `{"ok":false,"error":…}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct EngineDeltaEnvelope {
    /// Whether the command succeeded.
    pub ok: bool,
    /// Present when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub result: Option<EngineDelta>,
    /// Present when not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub error: Option<EngineError>,
}

impl DocumentApi {
    /// Like [`apply_json_envelope`](Self::apply_json_envelope), answering
    /// with an [`EngineDeltaEnvelope`] relative to the state before the
    /// command.
    pub fn apply_json_delta_envelope(&mut self, json: &str) -> String {
        self.delta_envelope(|api| api.apply_json(json))
    }

    /// Like [`apply_tool_envelope`](Self::apply_tool_envelope), answering
    /// with an [`EngineDeltaEnvelope`].
    pub fn apply_tool_delta_envelope(&mut self, tool: &str, args_json: &str) -> String {
        self.delta_envelope(|api| {
            let args =
                serde_json::from_str::<Value>(args_json).map_err(|e| CommandError::Invalid {
                    message: e.to_string(),
                })?;
            let cmd = crate::tools::lower(tool, &args, api)?;
            api.apply(cmd)
        })
    }

    fn delta_envelope(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<ApiResult, CommandError>,
    ) -> String {
        let before = self.get_state();
        let env = match f(self) {
            Ok(after) => EngineDeltaEnvelope {
                ok: true,
                result: Some(EngineDelta::between(&before, after)),
                error: None,
            },
            Err(e) => EngineDeltaEnvelope {
                ok: false,
                result: None,
                error: Some(EngineError::from(e)),
            },
        };
        serde_json::to_value(env).unwrap_or_default().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn document_delta_round_trips() {
        let prev =
            json!({"version":"0.1","nodes":{"1":{"a":1},"2":{"b":2}},"roots":[1],"old":true});
        let next = json!({"version":"0.1","nodes":{"1":{"a":1},"3":{"c":3}},"roots":[1,3],"materials":{"x":{}}});
        let d = DocumentDelta::between(&prev, &next);
        assert_eq!(d.maps["nodes"].set.keys().collect::<Vec<_>>(), ["3"]);
        assert_eq!(d.maps["nodes"].remove, ["2"]);
        assert_eq!(d.removed_fields, ["old"]);
        let mut doc = prev.clone();
        d.apply(&mut doc);
        assert_eq!(doc, next);
        assert!(DocumentDelta::between(&next, &next).is_empty());
    }

    #[test]
    fn editing_one_part_leaves_the_others_alone() {
        let mut api = DocumentApi::new(vcad_crdt::ReplicaId(1));
        let mut ids = Vec::new();
        for _ in 0..5 {
            let r = api.apply_json(
                r#"{"type":"add_feature","feature":{"type":"Cube","size_x":1,"size_y":1,"size_z":1}}"#,
            );
            ids.push(r.unwrap().created_feature_id.unwrap());
        }
        let out: Value = serde_json::from_str(&api.apply_json_delta_envelope(&format!(
            r#"{{"type":"set_translation","id":"{}","offset":[1,2,3]}}"#,
            ids[2]
        )))
        .unwrap();
        let delta: EngineDelta = serde_json::from_value(out["result"].clone()).unwrap();
        assert_eq!(delta.parts.updated.len(), 1);
        assert_eq!(delta.parts.updated[0].id(), ids[2]);
        assert!(delta.parts.added.is_empty() && delta.parts.removed.is_empty());
        assert_eq!(
            delta.document.maps["nodes"].set.len(),
            1,
            "only the translate node"
        );
        assert!(delta.parts.order.is_none());
    }
}
