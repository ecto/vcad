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
        /// Parser message.
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
    pub fn apply(&mut self, cmd: DocCommand) -> Result<ApiResult, CommandError> {
        if let DocCommand::Batch { commands } = cmd {
            return self.apply_batch(commands);
        }
        self.check(&cmd)?;
        Ok(self.apply_checked(cmd))
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
        let result = serde_json::from_str::<serde_json::Value>(args_json)
            .map_err(|e| CommandError::Invalid {
                message: e.to_string(),
            })
            .and_then(|args| crate::tools::lower(tool, &args))
            .and_then(|cmd| self.apply(cmd));
        envelope(result)
    }

    fn check(&self, cmd: &DocCommand) -> Result<(), CommandError> {
        for id in cmd.required_ids() {
            if self.stable_ids().resolve(id).is_none() {
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
            DocCommand::UpdateFeature { id, feature } => self.update_feature(&id, feature),
            DocCommand::SetParam { id, key, value } => self.set_param(&id, &key, value),
            DocCommand::DeleteFeature { id } => self.delete_feature(&id),
            DocCommand::SetTranslation { id, offset } => self.set_translation(&id, offset),
            DocCommand::SetRotation { id, angles } => self.set_rotation(&id, angles),
            DocCommand::SetScale { id, factor } => self.set_scale(&id, factor),
            DocCommand::SetMaterial { id, material } => self.set_material(&id, &material),
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

/// The JSON result envelope shared by every binding:
///
/// - `{"ok":true,"result":{"document":…,"parts":…,"consumedPartIds":…,"warnings":…,"createdFeatureId"?:…}}`
/// - `{"ok":false,"error":{"kind":"unknown_id",…,"message":"…"}}`
fn envelope(result: Result<ApiResult, CommandError>) -> String {
    let value = match result {
        Ok(r) => {
            let mut result = serde_json::json!({
                "document": r.document,
                "parts": r.parts,
                "consumedPartIds": r.consumed_part_ids,
                "warnings": r.warnings,
            });
            if let Some(id) = r.created_feature_id {
                result["createdFeatureId"] = id.into();
            }
            serde_json::json!({ "ok": true, "result": result })
        }
        Err(e) => {
            let mut error = serde_json::to_value(&e).unwrap_or_default();
            error["message"] = e.to_string().into();
            serde_json::json!({ "ok": false, "error": error })
        }
    };
    value.to_string()
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
