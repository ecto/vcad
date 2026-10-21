//! Document commands — the single mutation vocabulary.
//!
//! Every host (web via WASM, native via `vcad-ffi`, MCP, CLI, the AI
//! planner) mutates a document by sending a serialized [`DocCommand`] to
//! [`DocumentApi::apply`](crate::DocumentApi::apply). Hosts never compute
//! document state themselves; they render the [`ApiResult`] that comes back.
//!
//! Unlike the per-method `DocumentApi` calls (which silently no-op on an
//! unknown id), `apply` fails closed: an unknown stable id or an invalid
//! payload is a [`CommandError`], and the document is left untouched.

use serde::{Deserialize, Serialize};
use thiserror::Error;
use vcad_crdt::Value;

use crate::document_api::{ApiResult, DocumentApi};
use crate::feature::FeatureInput;
use crate::part_info::PartInfo;

/// One document mutation (or history step).
///
/// JSON shape: `{"type": "set_material", "id": "123:4", "material": "steel"}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocCommand {
    /// Append a new feature.
    AddFeature {
        /// Typed feature payload.
        feature: FeatureInput,
        /// Optional display name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-rs", ts(optional))]
        name: Option<String>,
    },
    /// Append a feature of a raw CRDT kind (`pcb-board`, `scene-settings`,
    /// …) for kinds without a typed [`FeatureInput`].
    CreateFeature {
        /// CRDT feature kind.
        kind: String,
        /// Initial params.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-rs", ts(type = "Record<string, CrdtValue>", optional))]
        params: Option<std::collections::HashMap<String, Value>>,
    },
    /// Replace an existing feature's params.
    UpdateFeature {
        /// Stable id of the feature.
        id: String,
        /// New typed payload.
        feature: FeatureInput,
    },
    /// Set one raw param on a feature.
    SetParam {
        /// Stable id of the feature.
        id: String,
        /// Param key.
        key: String,
        /// New value.
        value: Value,
    },
    /// Delete a feature.
    DeleteFeature {
        /// Stable id of the feature.
        id: String,
    },
    /// Set a feature's translation offset (mm).
    SetTranslation {
        /// Stable id of the feature.
        id: String,
        /// Offset along X, Y, Z.
        offset: [f64; 3],
    },
    /// Set a feature's rotation (degrees, XYZ).
    SetRotation {
        /// Stable id of the feature.
        id: String,
        /// Euler angles.
        angles: [f64; 3],
    },
    /// Set a feature's scale factor.
    SetScale {
        /// Stable id of the feature.
        id: String,
        /// Per-axis factor.
        factor: [f64; 3],
    },
    /// Assign a material preset.
    SetMaterial {
        /// Stable id of the feature.
        id: String,
        /// Material key.
        material: String,
    },
    /// Add or replace a definition in the document's `materials` table.
    DefineMaterial {
        /// Material key (what `set_material` references).
        key: String,
        /// The definition.
        #[cfg_attr(feature = "ts-rs", ts(type = "MaterialDef"))]
        definition: vcad_ir::MaterialDef,
    },
    /// Show or hide a feature.
    SetVisible {
        /// Stable id of the feature.
        id: String,
        /// Visibility.
        visible: bool,
    },
    /// Rename a feature.
    Rename {
        /// Stable id of the feature.
        id: String,
        /// New name.
        name: String,
    },
    /// Move a feature to a new index in the history.
    Reorder {
        /// Stable id of the feature.
        id: String,
        /// Target index.
        index: u32,
    },
    /// Duplicate features (appended at the end).
    Duplicate {
        /// Stable ids to copy.
        ids: Vec<String>,
    },
    /// Set a joint's state (degrees or mm).
    SetJointState {
        /// Stable id of the joint feature.
        id: String,
        /// New state.
        state: f64,
    },
    /// Merge an IR document into this one.
    ImportIr {
        /// IR `Document` as JSON.
        #[cfg_attr(feature = "ts-rs", ts(type = "Document"))]
        document: serde_json::Value,
    },
    /// Undo the last change.
    Undo,
    /// Redo the last undone change.
    Redo,
    /// Apply several commands as one batch, stopping at the first error.
    Batch {
        /// Commands, applied in order.
        commands: Vec<DocCommand>,
    },
}

/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Error, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CommandError {
    /// No feature has this stable id.
    #[error("no feature with id `{id}`")]
    UnknownId {
        /// The id that didn't resolve.
        id: String,
    },
    /// Nothing to undo or redo.
    #[error("nothing to {action}")]
    NothingTo {
        /// `undo` or `redo`.
        action: String,
    },
    /// The payload didn't parse.
    #[error("invalid command: {message}")]
    Invalid {
        /// Parser message. On the wire it is `detail`: the envelope's
        /// `message` (the display text) shares the error object.
        #[serde(rename = "detail")]
        message: String,
    },
    /// The host asked for a tool the engine can't lower yet.
    #[error("tool `{tool}` is not handled by the engine")]
    Unsupported {
        /// Tool or feature kind.
        tool: String,
    },
    /// A command inside a batch failed.
    #[error("batch command {index} failed: {source}")]
    Batch {
        /// Index of the failing command.
        index: usize,
        /// The underlying error.
        source: Box<CommandError>,
    },
}

impl DocCommand {
    /// Parse a command from JSON.
    pub fn from_json(json: &str) -> Result<Self, CommandError> {
        serde_json::from_str(json).map_err(|e| CommandError::Invalid {
            message: e.to_string(),
        })
    }

    /// Stable ids this command requires to exist.
    fn required_ids(&self) -> Vec<&str> {
        match self {
            Self::UpdateFeature { id, feature } => {
                let mut ids = vec![id.as_str()];
                ids.extend(feature.input_refs());
                ids
            }
            Self::SetParam { id, .. }
            | Self::DeleteFeature { id }
            | Self::SetTranslation { id, .. }
            | Self::SetRotation { id, .. }
            | Self::SetScale { id, .. }
            | Self::SetMaterial { id, .. }
            | Self::SetVisible { id, .. }
            | Self::Rename { id, .. }
            | Self::Reorder { id, .. }
            | Self::SetJointState { id, .. } => vec![id.as_str()],
            Self::Duplicate { ids } => ids.iter().map(String::as_str).collect(),
            Self::AddFeature { feature, .. } => feature.input_refs(),
            _ => Vec::new(),
        }
    }
}

impl DocumentApi {
    /// Apply one command. Fails closed: on error nothing is mutated
    /// (a failing [`DocCommand::Batch`] rolls back the commands before it).
    ///
    /// Every successful command is exactly one undo step, however many CRDT
    /// operations it took (a named add, a rotate about a pivot, a batch).
    pub fn apply(&mut self, cmd: DocCommand) -> Result<ApiResult, CommandError> {
        let history = matches!(cmd, DocCommand::Undo | DocCommand::Redo);
        let depth = self.crdt().undo_depth();
        let result = if let DocCommand::Batch { commands } = cmd {
            self.apply_batch(commands)
        } else {
            self.check(&cmd)?;
            Ok(self.apply_checked(cmd))
        };
        if result.is_ok() && !history {
            self.crdt_mut().coalesce_undo_since(depth);
        }
        result
    }

    /// Parse and apply a JSON command.
    pub fn apply_json(&mut self, json: &str) -> Result<ApiResult, CommandError> {
        self.apply(DocCommand::from_json(json)?)
    }

    /// Parse, apply, and wrap the outcome in the JSON envelope every
    /// binding returns, so wasm, ffi and CLI hosts see byte-identical
    /// results:
    ///
    /// - `{"ok":true,"result":{"document":…,"parts":…,"consumedPartIds":…,"warnings":…,"createdFeatureId"?:…}}`
    /// - `{"ok":false,"error":{"kind":"unknown_id",…,"message":"…"}}`
    pub fn apply_json_envelope(&mut self, json: &str) -> String {
        let result = self.apply_json(json);
        envelope(result)
    }

    /// Lower an AI tool call ([`crate::tools::lower`]) and apply it,
    /// returning the same envelope as [`apply_json_envelope`](Self::apply_json_envelope).
    pub fn apply_tool_envelope(&mut self, tool: &str, args_json: &str) -> String {
        let lowered = serde_json::from_str::<serde_json::Value>(args_json)
            .map_err(|e| CommandError::Invalid {
                message: e.to_string(),
            })
            .and_then(|args| crate::tools::lower(tool, &args, self));
        envelope(lowered.and_then(|cmd| self.apply(cmd)))
    }

    /// The current state in the same envelope as
    /// [`apply_json_envelope`](Self::apply_json_envelope) (always `ok`).
    pub fn state_envelope(&self) -> String {
        envelope(Ok(self.get_state()))
    }

    fn check(&self, cmd: &DocCommand) -> Result<(), CommandError> {
        for id in cmd.required_ids() {
            if !self.feature_exists(id) {
                return Err(CommandError::UnknownId { id: id.to_string() });
            }
        }
        match cmd {
            DocCommand::Undo if !self.can_undo() => Err(CommandError::NothingTo {
                action: "undo".into(),
            }),
            DocCommand::Redo if !self.can_redo() => Err(CommandError::NothingTo {
                action: "redo".into(),
            }),
            DocCommand::ImportIr { document } => {
                serde_json::from_value::<vcad_ir::Document>(document.clone())
                    .map(|_| ())
                    .map_err(|e| CommandError::Invalid {
                        message: format!("import_ir: {e}"),
                    })
            }
            _ => Ok(()),
        }
    }

    fn apply_checked(&mut self, cmd: DocCommand) -> ApiResult {
        match cmd {
            DocCommand::AddFeature { feature, name } => {
                let result = self.add_feature(feature);
                match (name, result.created_feature_id.clone()) {
                    (Some(name), Some(id)) => {
                        let mut renamed = self.rename_feature(&id, &name);
                        renamed.created_feature_id = Some(id);
                        renamed
                    }
                    _ => result,
                }
            }
            DocCommand::CreateFeature { kind, params } => {
                self.create_feature_raw(&kind, params.unwrap_or_default())
            }
            DocCommand::UpdateFeature { id, feature } => self.update_feature(&id, feature),
            DocCommand::SetParam { id, key, value } => self.set_param(&id, &key, value),
            DocCommand::DeleteFeature { id } => self.delete_feature(&id),
            DocCommand::SetTranslation { id, offset } => self.set_translation(&id, offset),
            DocCommand::SetRotation { id, angles } => self.set_rotation(&id, angles),
            DocCommand::SetScale { id, factor } => self.set_scale(&id, factor),
            DocCommand::SetMaterial { id, material } => self.set_material(&id, &material),
            DocCommand::DefineMaterial { key, definition } => {
                self.define_material(&key, &definition)
            }
            DocCommand::SetVisible { id, visible } => self.set_visible(&id, visible),
            DocCommand::Rename { id, name } => self.rename_feature(&id, &name),
            DocCommand::Reorder { id, index } => self.reorder_feature(&id, index),
            DocCommand::Duplicate { ids } => self.duplicate_features(&ids),
            DocCommand::SetJointState { id, state } => self.set_joint_state(&id, state),
            DocCommand::ImportIr { document } => self.import_ir(&document.to_string()),
            DocCommand::Undo => self.undo(),
            DocCommand::Redo => self.redo(),
            DocCommand::Batch { commands } => self
                .apply_batch(commands)
                .unwrap_or_else(|_| self.get_state()),
        }
    }

    fn apply_batch(&mut self, commands: Vec<DocCommand>) -> Result<ApiResult, CommandError> {
        let snapshot = self.save();
        let mut last = self.get_state();
        for (index, cmd) in commands.into_iter().enumerate() {
            match self.apply(cmd) {
                Ok(result) => last = result,
                Err(source) => {
                    self.restore(&snapshot);
                    return Err(CommandError::Batch {
                        index,
                        source: Box::new(source),
                    });
                }
            }
        }
        Ok(last)
    }
}

/// Successful result payload inside an [`EngineEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
#[serde(rename_all = "camelCase")]
pub struct EngineResult {
    /// The materialized IR document.
    #[cfg_attr(feature = "ts-rs", ts(type = "Document"))]
    pub document: vcad_ir::Document,
    /// Part metadata per materialized feature.
    pub parts: Vec<PartInfo>,
    /// Stable ids of parts consumed by other features.
    pub consumed_part_ids: Vec<String>,
    /// Non-fatal materialize issues.
    pub warnings: Vec<String>,
    /// Stable id of the feature the command created, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub created_feature_id: Option<String>,
}

/// A refused command inside an [`EngineEnvelope`]: the [`CommandError`]
/// fields plus its display message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct EngineError {
    /// The structured error.
    #[serde(flatten)]
    pub error: CommandError,
    /// Human-readable message.
    pub message: String,
}

/// The JSON result envelope shared by every binding (wasm, ffi, CLI):
///
/// - `{"ok":true,"result":{"document":…,"parts":…,"consumedPartIds":…,"warnings":…,"createdFeatureId"?:…}}`
/// - `{"ok":false,"error":{"kind":"unknown_id",…,"message":"…"}}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct EngineEnvelope {
    /// Whether the command succeeded.
    pub ok: bool,
    /// Present when `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub result: Option<EngineResult>,
    /// Present when not `ok`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub error: Option<EngineError>,
}

/// An AI tool call as hosts hand it to the engine
/// ([`DocumentApi::apply_tool_envelope`]). `args` stays JSON: each tool
/// lowers its own argument shape in [`crate::tools::lower`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct ToolCall {
    /// Tool name (`create`, `translate`, `place`, …).
    pub tool: String,
    /// Tool arguments (a JSON object).
    #[cfg_attr(feature = "ts-rs", ts(type = "Record<string, unknown>"))]
    pub args: serde_json::Value,
}

impl From<CommandError> for EngineError {
    fn from(error: CommandError) -> Self {
        Self {
            message: error.to_string(),
            error,
        }
    }
}

impl From<Result<ApiResult, CommandError>> for EngineEnvelope {
    fn from(result: Result<ApiResult, CommandError>) -> Self {
        match result {
            Ok(r) => Self {
                ok: true,
                result: Some(EngineResult {
                    document: r.document,
                    parts: r.parts,
                    consumed_part_ids: r.consumed_part_ids,
                    warnings: r.warnings,
                    created_feature_id: r.created_feature_id,
                }),
                error: None,
            },
            Err(e) => Self {
                ok: false,
                result: None,
                error: Some(EngineError::from(e)),
            },
        }
    }
}

/// Serialize the [`EngineEnvelope`] for `result`. Goes through
/// `serde_json::Value` so object keys come out sorted (stable bytes across
/// hosts, whatever order the IR's maps iterate in).
fn envelope(result: Result<ApiResult, CommandError>) -> String {
    serde_json::to_value(EngineEnvelope::from(result))
        .unwrap_or_default()
        .to_string()
}

#[cfg(all(test, feature = "ts-rs"))]
mod ts_export {
    use super::*;
    use ts_rs::TS;

    /// Generate TypeScript definitions for the command vocabulary
    /// (bundled into `packages/ir/src/generated.ts` by `npm run ir:gen`).
    #[test]
    #[ignore = "requires --features ts-rs; produces bindings/ output, opt-in only"]
    fn export_bindings() {
        DocCommand::export_all().expect("DocCommand export failed");
        CommandError::export_all().expect("CommandError export failed");
        FeatureInput::export_all().expect("FeatureInput export failed");
        crate::feature::BooleanType::export_all().expect("BooleanType export failed");
        Value::export_all().expect("Value export failed");
        EngineEnvelope::export_all().expect("EngineEnvelope export failed");
        ToolCall::export_all().expect("ToolCall export failed");
        crate::delta::EngineDeltaEnvelope::export_all().expect("EngineDeltaEnvelope export failed");
        // Referenced only through a `ts(type)` override, so not pulled in above.
        crate::delta::MapPatch::export_all().expect("MapPatch export failed");
        crate::agent::AgentManifest::export_all().expect("AgentManifest export failed");
        crate::agent::AgentReadEnvelope::export_all().expect("AgentReadEnvelope export failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_crdt::ReplicaId;

    fn cube() -> FeatureInput {
        FeatureInput::Cube {
            size_x: 10.0,
            size_y: 20.0,
            size_z: 30.0,
        }
    }

    fn api() -> DocumentApi {
        DocumentApi::new(ReplicaId(1))
    }

    #[test]
    fn add_feature_from_json_creates_named_part() {
        let mut api = api();
        let res = api
            .apply_json(
                r#"{"type":"add_feature","name":"Plate",
                    "feature":{"type":"Cube","size_x":10,"size_y":20,"size_z":30}}"#,
            )
            .unwrap();
        let id = res.created_feature_id.expect("created id");
        assert_eq!(res.parts.len(), 1);
        assert_eq!(
            serde_json::to_value(&res.parts[0]).unwrap()["name"],
            "Plate"
        );
        assert!(api.stable_ids().resolve(&id).is_some());
    }

    #[test]
    fn unknown_id_is_refused() {
        let mut api = api();
        let err = api
            .apply(DocCommand::SetMaterial {
                id: "9:9".into(),
                material: "steel".into(),
            })
            .unwrap_err();
        assert_eq!(err, CommandError::UnknownId { id: "9:9".into() });
    }

    #[test]
    fn invalid_json_is_refused() {
        let mut api = api();
        let err = api.apply_json(r#"{"type":"teleport"}"#).unwrap_err();
        assert!(matches!(err, CommandError::Invalid { .. }));
    }

    #[test]
    fn undo_with_empty_history_is_refused() {
        let mut api = api();
        assert!(matches!(
            api.apply(DocCommand::Undo),
            Err(CommandError::NothingTo { .. })
        ));
    }

    #[test]
    fn transform_then_undo_round_trips() {
        let mut api = api();
        let id = api
            .apply(DocCommand::AddFeature {
                feature: cube(),
                name: None,
            })
            .unwrap()
            .created_feature_id
            .unwrap();
        api.apply(DocCommand::SetTranslation {
            id: id.clone(),
            offset: [5.0, 0.0, 0.0],
        })
        .unwrap();
        let undone = api.apply(DocCommand::Undo).unwrap();
        assert_eq!(undone.parts.len(), 1);
        assert!(api.can_redo());
    }

    #[test]
    fn failing_batch_rolls_back() {
        let mut api = api();
        let err = api
            .apply(DocCommand::Batch {
                commands: vec![
                    DocCommand::AddFeature {
                        feature: cube(),
                        name: None,
                    },
                    DocCommand::DeleteFeature { id: "nope".into() },
                ],
            })
            .unwrap_err();
        assert!(matches!(err, CommandError::Batch { index: 1, .. }));
        assert_eq!(api.get_state().parts.len(), 0);
    }

    #[test]
    fn envelope_reports_success_and_errors() {
        let mut api = api();
        let ok: serde_json::Value = serde_json::from_str(&api.apply_json_envelope(
            r#"{"type":"add_feature","feature":{"type":"Cube","size_x":1,"size_y":1,"size_z":1}}"#,
        ))
        .unwrap();
        assert_eq!(ok["ok"], true);
        assert!(ok["result"]["createdFeatureId"].is_string());

        let err: serde_json::Value =
            serde_json::from_str(&api.apply_json_envelope(r#"{"type":"delete_feature","id":"x"}"#))
                .unwrap();
        assert_eq!(err["ok"], false);
        assert_eq!(err["error"]["kind"], "unknown_id");
        assert_eq!(err["error"]["message"], "no feature with id `x`");
    }

    #[test]
    fn deleted_features_are_unknown() {
        let mut api = api();
        let id = api
            .apply(DocCommand::AddFeature {
                feature: cube(),
                name: None,
            })
            .unwrap()
            .created_feature_id
            .unwrap();
        api.apply(DocCommand::DeleteFeature { id: id.clone() })
            .unwrap();
        assert_eq!(
            api.apply(DocCommand::DeleteFeature { id: id.clone() })
                .unwrap_err(),
            CommandError::UnknownId { id }
        );
    }

    #[test]
    fn create_feature_raw_then_set_param() {
        let mut api = api();
        let id = api
            .apply_json(r#"{"type":"create_feature","kind":"scene-settings"}"#)
            .unwrap()
            .created_feature_id
            .unwrap();
        api.apply_json(&format!(
            r#"{{"type":"set_param","id":"{id}","key":"background","value":{{"String":"white"}}}}"#
        ))
        .unwrap();
    }

    #[test]
    fn reorder_moves_down_to_the_requested_index() {
        let mut api = api();
        let ids: Vec<String> = (0..3)
            .map(|_| {
                api.apply(DocCommand::AddFeature {
                    feature: cube(),
                    name: None,
                })
                .unwrap()
                .created_feature_id
                .unwrap()
            })
            .collect();
        api.apply(DocCommand::Reorder {
            id: ids[0].clone(),
            index: 1,
        })
        .unwrap();
        let order: Vec<String> = api
            .crdt()
            .ordered_features()
            .iter()
            .map(|(fid, _)| api.stable_ids().stable_id(*fid).unwrap().to_string())
            .collect();
        assert_eq!(order, [ids[1].clone(), ids[0].clone(), ids[2].clone()]);
    }

    fn steel() -> vcad_ir::MaterialDef {
        serde_json::from_str(r#"{"name":"steel","color":[0.6,0.6,0.65],"metallic":1.0,"roughness":0.4,"density":7850.0}"#)
            .unwrap()
    }

    #[test]
    fn define_material_adds_to_materials_and_survives_reload() {
        let mut api = api();
        let id = api
            .apply(DocCommand::AddFeature {
                feature: cube(),
                name: None,
            })
            .unwrap()
            .created_feature_id
            .unwrap();
        let cmd = DocCommand::from_json(
            &serde_json::json!({"type":"define_material","key":"steel","definition":steel()})
                .to_string(),
        )
        .unwrap();
        let r = api.apply(cmd).unwrap();
        assert!(r.created_feature_id.is_none());
        assert_eq!(r.document.materials["steel"].density, Some(7850.0));
        let r = api
            .apply(DocCommand::SetMaterial {
                id,
                material: "steel".into(),
            })
            .unwrap();
        assert_eq!(r.document.roots[0].material, "steel");

        // Saved and reopened, the definition is still there — and defining a
        // second key on the imported doc keeps the first.
        let saved = serde_json::to_string(&r.document).unwrap();
        let mut reopened = DocumentApi::from_ir(&saved, ReplicaId(9)).unwrap();
        let mut abs = steel();
        abs.name = "abs".into();
        let r = reopened
            .apply(DocCommand::DefineMaterial {
                key: "abs".into(),
                definition: abs,
            })
            .unwrap();
        let mut keys: Vec<_> = r.document.materials.keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["abs", "steel"]);
        assert_eq!(r.document.roots[0].material, "steel");

        // One undo step removes just the new definition.
        let r = reopened.apply(DocCommand::Undo).unwrap();
        assert!(!r.document.materials.contains_key("abs"));
        assert!(r.document.materials.contains_key("steel"));
    }

    #[test]
    fn envelope_shape_is_stable() {
        let mut api = api();
        let ok: serde_json::Value = serde_json::from_str(&api.apply_json_envelope(
            r#"{"type":"add_feature","feature":{"type":"Sphere","radius":1}}"#,
        ))
        .unwrap();
        assert_eq!(ok["ok"], true);
        assert!(ok["result"]["createdFeatureId"].is_string());
        assert!(ok["result"]["consumedPartIds"].is_array());
        assert!(ok.get("error").is_none());
        let err = api.apply_json_envelope(r#"{"type":"delete_feature","id":"9:9"}"#);
        assert_eq!(
            err,
            r#"{"error":{"id":"9:9","kind":"unknown_id","message":"no feature with id `9:9`"},"ok":false}"#
        );
        let env: EngineEnvelope = serde_json::from_str(&err).unwrap();
        assert!(matches!(
            env.error.unwrap().error,
            CommandError::UnknownId { .. }
        ));
    }

    #[test]
    fn invalid_error_round_trips_through_the_envelope() {
        let mut api = api();
        let raw = api.apply_json_envelope(r#"{"type":"nope"}"#);
        let env: EngineEnvelope = serde_json::from_str(&raw).unwrap();
        let err = env.error.unwrap();
        let CommandError::Invalid { message } = &err.error else {
            panic!("expected invalid, got {err:?}");
        };
        assert!(err.message.starts_with("invalid command: "));
        assert_eq!(err.message, format!("invalid command: {message}"));
        let again = serde_json::to_value(EngineEnvelope {
            ok: false,
            result: None,
            error: Some(err),
        })
        .unwrap();
        assert_eq!(
            again,
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()
        );
    }

    #[test]
    fn command_json_shape_is_stable() {
        let cmd = DocCommand::SetMaterial {
            id: "1:0".into(),
            material: "steel".into(),
        };
        assert_eq!(
            serde_json::to_string(&cmd).unwrap(),
            r#"{"type":"set_material","id":"1:0","material":"steel"}"#
        );
    }
}
