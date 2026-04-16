//! CRUD tool executor — the Rust side of the chat tool contract.
//!
//! Split into two layers so every frontend can reuse the validation +
//! argument parsing without being forced into the same mutation machinery:
//!
//! - [`plan_crud`] takes a `&Document` read-only and returns a
//!   [`PlannedResponse`] describing what should happen via a
//!   [`ToolOutcome`]. No mutation. Used by the web's TS executor, which
//!   dispatches the outcome through the CRDT engine.
//! - [`apply_outcome`] takes a `&mut Document` and applies the planned
//!   outcome via direct struct mutation. Used by the TUI and by
//!   [`execute_crud`], which is now a thin wrapper for the
//!   `plan → apply` flow.
//!
//! Tools:
//! - **create**       — builds a `ToolOutcome::AddFeature` via `CsgOp` serde roundtrip.
//! - **read**         — pure, no outcome — returns the description string.
//! - **update**       — builds a `ToolOutcome::UpdateParams` to merge into a node.
//! - **delete**       — builds a `ToolOutcome::RemovePart`.
//! - **set_material** — builds a `ToolOutcome::SetPartMaterial`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry};

/// The status of a tool execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExecutionStatus {
    Success,
    Error,
}

/// Structured result of an executor call. Mirrors `ExecutionResult` in
/// `packages/core/src/commands/types.ts`, minus the `display` / `duration`
/// fields which the chat panel derives locally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub status: ExecutionStatus,
    /// Human-readable summary returned to the model.
    pub result: String,
    /// Part id if a part was created or modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_id: Option<String>,
    /// Node id if a node was created or modified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

impl ExecutionResult {
    pub fn success(result: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Success,
            result: result.into(),
            part_id: None,
            node_id: None,
        }
    }

    pub fn error(result: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Error,
            result: result.into(),
            part_id: None,
            node_id: None,
        }
    }

    pub fn with_part(mut self, part_id: impl Into<String>) -> Self {
        self.part_id = Some(part_id.into());
        self
    }

    pub fn with_node(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }
}

// ---------------------------------------------------------------------------
// Plan + apply split
// ---------------------------------------------------------------------------

/// What a tool call should do, independent of how a particular frontend
/// applies it. The web dispatches this through its CRDT engine methods
/// (`add_feature` / `setFeatureParam` / `removePart` / `setPartMaterial`);
/// the TUI applies it in-place via [`apply_outcome`].
///
/// Note: `AddFeature` does not carry a node id. Each applier assigns its
/// own id — the TUI picks `max(existing ids) + 1`, the CRDT engine
/// returns its own id from `add_feature`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolOutcome {
    /// Insert a new feature. `parent_part_id`, when set, rewires an
    /// existing scene-entry root to the new node (used by modifiers
    /// like Fillet / Shell that wrap an existing solid).
    AddFeature {
        op: CsgOp,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        parent_part_id: Option<String>,
    },
    /// Merge partial params into an existing node's op. The applier
    /// handles the serde roundtrip (web can also dispatch per-field via
    /// `setFeatureParam`).
    UpdateParams {
        node_id: String,
        params: Value,
    },
    /// Remove a part and its root node from the document.
    RemovePart { part_id: String },
    /// Assign a material preset to a part's scene entry.
    SetPartMaterial { part_id: String, material: String },
}

/// Read-only planner response. `result` is the human-facing summary the
/// chat panel renders (and the model sees on the next turn). `outcome`
/// is `None` for pure reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedResponse {
    pub status: ExecutionStatus,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ToolOutcome>,
}

impl PlannedResponse {
    fn success(result: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Success,
            result: result.into(),
            outcome: None,
        }
    }

    fn error(result: impl Into<String>) -> Self {
        Self {
            status: ExecutionStatus::Error,
            result: result.into(),
            outcome: None,
        }
    }

    fn with_outcome(mut self, outcome: ToolOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }
}

/// Plan a tool call against a read-only document. Returns what should
/// happen without mutating anything. Each frontend feeds the resulting
/// [`ToolOutcome`] into its own mutation path.
pub fn plan_crud(tool: &str, args: &Value, doc: &Document) -> PlannedResponse {
    match tool {
        "create" => plan_create(args, doc),
        "read" => plan_read(args, doc),
        "update" => plan_update(args, doc),
        "delete" => plan_delete(args, doc),
        "set_material" => plan_set_material(args, doc),
        other => PlannedResponse::error(format!("unknown tool: {other}")),
    }
}

/// Apply a planned outcome to a `&mut Document`. Returns the id assigned
/// to a newly-created node (for `AddFeature`), or a matched id for other
/// outcomes so the caller can tell the model which entity was affected.
///
/// The web's CRDT-backed docstore uses its own applier that dispatches
/// through `engine.add_feature` etc.; this function is for every other
/// frontend that mutates a `vcad_ir::Document` directly.
pub fn apply_outcome(
    doc: &mut Document,
    outcome: &ToolOutcome,
) -> Result<ApplyResult, String> {
    match outcome {
        ToolOutcome::AddFeature {
            op,
            name,
            parent_part_id,
        } => {
            let new_id = next_node_id(doc);
            doc.nodes.insert(
                new_id,
                Node {
                    id: new_id,
                    name: name.clone(),
                    op: op.clone(),
                },
            );
            if let Some(parent) = parent_part_id {
                let parent_nid = parent
                    .parse::<NodeId>()
                    .map_err(|_| format!("invalid parent_part_id: {parent}"))?;
                let entry = doc
                    .roots
                    .iter_mut()
                    .find(|e| e.root == parent_nid)
                    .ok_or_else(|| format!("parent part not found: {parent}"))?;
                entry.root = new_id;
            } else {
                doc.roots.push(SceneEntry {
                    root: new_id,
                    material: "default".to_string(),
                    visible: None,
                });
            }
            Ok(ApplyResult {
                part_id: Some(new_id.to_string()),
                node_id: Some(new_id.to_string()),
            })
        }
        ToolOutcome::UpdateParams { node_id, params } => {
            let nid = node_id
                .parse::<NodeId>()
                .map_err(|_| format!("invalid node_id: {node_id}"))?;
            let node = doc
                .nodes
                .get_mut(&nid)
                .ok_or_else(|| format!("node not found: {node_id}"))?;
            let mut op_value = serde_json::to_value(&node.op)
                .map_err(|e| format!("failed to serialize op: {e}"))?;
            let op_map = op_value
                .as_object_mut()
                .ok_or_else(|| "current op is not an object — unexpected shape".to_string())?;
            if let Value::Object(incoming) = params {
                for (key, val) in incoming {
                    if key == "type" {
                        continue;
                    }
                    op_map.insert(key.clone(), val.clone());
                }
            }
            let new_op: CsgOp = serde_json::from_value(op_value)
                .map_err(|e| format!("failed to apply params to {node_id}: {e}"))?;
            node.op = new_op;
            Ok(ApplyResult {
                part_id: None,
                node_id: Some(node_id.clone()),
            })
        }
        ToolOutcome::RemovePart { part_id } => {
            let nid = part_id
                .parse::<NodeId>()
                .map_err(|_| format!("invalid part_id: {part_id}"))?;
            let before = doc.roots.len();
            doc.roots.retain(|e| e.root != nid);
            if doc.roots.len() == before {
                return Err(format!("part not found: {part_id}"));
            }
            doc.nodes.remove(&nid);
            Ok(ApplyResult {
                part_id: Some(part_id.clone()),
                node_id: None,
            })
        }
        ToolOutcome::SetPartMaterial { part_id, material } => {
            let nid = part_id
                .parse::<NodeId>()
                .map_err(|_| format!("invalid part_id: {part_id}"))?;
            let entry = doc
                .roots
                .iter_mut()
                .find(|e| e.root == nid)
                .ok_or_else(|| format!("part not found: {part_id}"))?;
            entry.material = material.clone();
            Ok(ApplyResult {
                part_id: Some(part_id.clone()),
                node_id: None,
            })
        }
    }
}

/// Identifiers a successful [`apply_outcome`] call resolved or assigned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplyResult {
    pub part_id: Option<String>,
    pub node_id: Option<String>,
}

/// Dispatch a CRUD tool call and apply it to the document in one shot.
/// Thin wrapper over [`plan_crud`] + [`apply_outcome`] — the split is
/// the seam the web uses to dispatch the outcome through its CRDT
/// engine instead of mutating the IR document directly.
pub fn execute_crud(tool: &str, args: &Value, doc: &mut Document) -> ExecutionResult {
    let planned = plan_crud(tool, args, doc);
    let Some(outcome) = planned.outcome.clone() else {
        // Pure read or planner error — no mutation.
        return ExecutionResult {
            status: planned.status,
            result: planned.result,
            part_id: None,
            node_id: None,
        };
    };
    if planned.status == ExecutionStatus::Error {
        return ExecutionResult {
            status: planned.status,
            result: planned.result,
            part_id: None,
            node_id: None,
        };
    }

    match apply_outcome(doc, &outcome) {
        Ok(ids) => {
            // Augment the result with the assigned id so follow-up tool
            // calls can reference the new part. The planner's result is
            // id-agnostic ("Created cube"); we append the concrete id
            // the applier picked.
            let augmented = match (&outcome, &ids.node_id) {
                (ToolOutcome::AddFeature { .. }, Some(nid)) => {
                    format!("{} with id: {nid}", planned.result)
                }
                _ => planned.result,
            };
            ExecutionResult {
                status: ExecutionStatus::Success,
                result: augmented,
                part_id: ids.part_id,
                node_id: ids.node_id,
            }
        }
        Err(e) => ExecutionResult::error(e),
    }
}

// ---------------------------------------------------------------------------
// plan: create
// ---------------------------------------------------------------------------

fn plan_create(args: &Value, _doc: &Document) -> PlannedResponse {
    let Some(ty) = args.get("type").and_then(|v| v.as_str()) else {
        return PlannedResponse::error("create requires `type`");
    };
    let params = args
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let name = args.get("name").and_then(|v| v.as_str()).map(str::to_string);
    let parent_part_id = args
        .get("parent_part_id")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    // CsgOp carries `#[serde(tag = "type")]` with no rename_all, so serde
    // wants the PascalCase variant name ("Cube") — but the ToolSchema proc
    // macro publishes snake_case names ("cube"). Bridge the two before
    // handing off to serde.
    let pascal_ty = snake_to_pascal(ty);

    let mut obj = Map::new();
    obj.insert("type".into(), Value::String(pascal_ty));
    if let Value::Object(p) = params {
        for (k, v) in p {
            obj.insert(k, v);
        }
    } else {
        return PlannedResponse::error("create `params` must be an object");
    }

    let op: CsgOp = match serde_json::from_value(Value::Object(obj)) {
        Ok(op) => op,
        Err(e) => return PlannedResponse::error(format!("failed to parse {ty}: {e}")),
    };

    // Planner result is id-agnostic — the applier appends the concrete
    // id it picked once the node actually lands.
    let summary = if parent_part_id.is_some() {
        format!("Wrapped part with {ty}")
    } else {
        format!("Created {ty}")
    };

    PlannedResponse::success(summary).with_outcome(ToolOutcome::AddFeature {
        op,
        name,
        parent_part_id,
    })
}

/// Compute the next free NodeId by taking max + 1 over the existing keys.
fn next_node_id(doc: &Document) -> NodeId {
    doc.nodes.keys().copied().max().unwrap_or(0) + 1
}

/// Convert a snake_case identifier to PascalCase.
/// `"linear_pattern"` → `"LinearPattern"`, `"cube"` → `"Cube"`.
fn snake_to_pascal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper_next = true;
    for ch in s.chars() {
        if ch == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(ch.to_uppercase());
            upper_next = false;
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// plan: read
// ---------------------------------------------------------------------------

fn plan_read(args: &Value, doc: &Document) -> PlannedResponse {
    // Read is pure — delegate to the existing impl which already returns
    // an id-agnostic result string.
    let er = execute_read_inner(args, doc);
    PlannedResponse {
        status: er.status,
        result: er.result,
        outcome: None,
    }
}

fn execute_read_inner(args: &Value, doc: &Document) -> ExecutionResult {
    let part_id = args.get("part_id").and_then(|v| v.as_str());
    match part_id {
        None => {
            if doc.roots.is_empty() {
                return ExecutionResult::success("Document is empty — no parts yet.");
            }
            let mut lines = vec![format!("{} parts:", doc.roots.len())];
            for entry in &doc.roots {
                let node = doc.nodes.get(&entry.root);
                let name = node
                    .and_then(|n| n.name.as_deref())
                    .unwrap_or("(unnamed)");
                let kind = node
                    .map(|n| variant_name(&n.op))
                    .unwrap_or_else(|| "unknown".to_string());
                lines.push(format!(
                    "- {}: \"{}\" [{}] material={}",
                    entry.root, name, kind, entry.material
                ));
            }
            ExecutionResult::success(lines.join("\n"))
        }
        Some(id) => {
            let Ok(nid) = id.parse::<NodeId>() else {
                return ExecutionResult::error(format!("invalid part id: {id}"));
            };
            let Some(entry) = doc.roots.iter().find(|e| e.root == nid) else {
                return ExecutionResult::error(format!("part not found: {id}"));
            };
            let Some(node) = doc.nodes.get(&nid) else {
                return ExecutionResult::error(format!("node missing for part: {id}"));
            };
            let params_json = serde_json::to_string_pretty(&node.op).unwrap_or_default();
            ExecutionResult::success(format!(
                "Part {}\n  name: {}\n  kind: {}\n  material: {}\n  params:\n{}",
                nid,
                node.name.as_deref().unwrap_or("(unnamed)"),
                variant_name(&node.op),
                entry.material,
                indent(&params_json, 4)
            ))
            .with_part(nid.to_string())
            .with_node(nid.to_string())
        }
    }
}

fn indent(s: &str, spaces: usize) -> String {
    let pad: String = " ".repeat(spaces);
    s.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce the snake_case variant name of a CsgOp for display. We use serde
/// to recover the `type` tag (which is PascalCase since CsgOp has no
/// `rename_all`) and normalize it so model-facing text matches the schema
/// names published by `ToolSchema` / `schemas::all_schemas`.
fn variant_name(op: &CsgOp) -> String {
    let pascal = serde_json::to_value(op)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .unwrap_or_else(|| "CsgOp".to_string());
    pascal_to_snake(&pascal)
}

/// Convert a PascalCase identifier to snake_case.
/// `"LinearPattern"` → `"linear_pattern"`, `"Cube"` → `"cube"`.
fn pascal_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

// ---------------------------------------------------------------------------
// plan: update
// ---------------------------------------------------------------------------

fn plan_update(args: &Value, doc: &Document) -> PlannedResponse {
    let Some(node_id_str) = args.get("node_id").and_then(|v| v.as_str()) else {
        return PlannedResponse::error("update requires `node_id`");
    };
    let Ok(nid) = node_id_str.parse::<NodeId>() else {
        return PlannedResponse::error(format!("invalid node_id: {node_id_str}"));
    };
    let Some(params_value) = args.get("params") else {
        return PlannedResponse::error("update requires `params`");
    };
    if !matches!(params_value, Value::Object(_)) {
        return PlannedResponse::error("update `params` must be an object");
    }

    // Verify the node exists and the merge would succeed — this is the
    // "dry run" planner, so we do a serde round-trip against a clone
    // without touching the document.
    let Some(node) = doc.nodes.get(&nid) else {
        return PlannedResponse::error(format!("node not found: {node_id_str}"));
    };
    let mut op_value = match serde_json::to_value(&node.op) {
        Ok(v) => v,
        Err(e) => return PlannedResponse::error(format!("failed to serialize op: {e}")),
    };
    let Value::Object(op_map) = &mut op_value else {
        return PlannedResponse::error("current op is not an object — unexpected shape");
    };
    if let Value::Object(incoming) = params_value {
        for (key, val) in incoming {
            if key == "type" {
                continue;
            }
            op_map.insert(key.clone(), val.clone());
        }
    }
    if let Err(e) = serde_json::from_value::<CsgOp>(op_value) {
        return PlannedResponse::error(format!("failed to apply params to {node_id_str}: {e}"));
    }

    PlannedResponse::success(format!("Updated node {node_id_str}")).with_outcome(
        ToolOutcome::UpdateParams {
            node_id: node_id_str.to_string(),
            params: params_value.clone(),
        },
    )
}

// ---------------------------------------------------------------------------
// plan: delete
// ---------------------------------------------------------------------------

fn plan_delete(args: &Value, doc: &Document) -> PlannedResponse {
    let Some(part_id) = args.get("part_id").and_then(|v| v.as_str()) else {
        return PlannedResponse::error("delete requires `part_id`");
    };
    if part_id.is_empty() {
        return PlannedResponse::error("delete requires a non-empty part_id");
    }
    // Validate: part_id may be a numeric NodeId (TUI) or a CRDT stable_id
    // like "1:0" (web). Check both formats.
    let found = if let Ok(nid) = part_id.parse::<NodeId>() {
        doc.roots.iter().any(|e| e.root == nid)
    } else {
        // Stable-id — can't validate against the IR document's NodeId-based
        // roots, so trust it and let the CRDT engine handle resolution.
        true
    };
    if !found {
        return PlannedResponse::error(format!("part not found: {part_id}"));
    }
    PlannedResponse::success(format!("Deleted part {part_id}")).with_outcome(
        ToolOutcome::RemovePart {
            part_id: part_id.to_string(),
        },
    )
}

// ---------------------------------------------------------------------------
// plan: set_material
// ---------------------------------------------------------------------------

fn plan_set_material(args: &Value, doc: &Document) -> PlannedResponse {
    let Some(part_id) = args.get("part_id").and_then(|v| v.as_str()) else {
        return PlannedResponse::error("set_material requires `part_id`");
    };
    let Some(material) = args.get("material").and_then(|v| v.as_str()) else {
        return PlannedResponse::error("set_material requires `material`");
    };
    if part_id.is_empty() {
        return PlannedResponse::error("set_material requires a non-empty part_id");
    }
    // Validate: part_id may be a numeric NodeId (TUI) or a CRDT stable_id
    // like "1:0" (web). Check both formats.
    let found = if let Ok(nid) = part_id.parse::<NodeId>() {
        doc.roots.iter().any(|e| e.root == nid)
    } else {
        true
    };
    if !found {
        return PlannedResponse::error(format!("part not found: {part_id}"));
    }
    PlannedResponse::success(format!("Set {part_id} material to {material}")).with_outcome(
        ToolOutcome::SetPartMaterial {
            part_id: part_id.to_string(),
            material: material.to_string(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn empty_doc() -> Document {
        Document::new()
    }

    #[test]
    fn create_cube_adds_node_and_root() {
        let mut doc = empty_doc();
        let args = json!({
            "type": "cube",
            "params": { "size": { "x": 20, "y": 20, "z": 20 } },
            "name": "Test Cube"
        });
        let res = execute_crud("create", &args, &mut doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.result.contains("Created cube"));
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.roots.len(), 1);
        let node = doc.nodes.values().next().unwrap();
        assert_eq!(node.name.as_deref(), Some("Test Cube"));
        assert!(matches!(node.op, CsgOp::Cube { .. }));
    }

    #[test]
    fn create_rejects_bad_type() {
        let mut doc = empty_doc();
        let args = json!({ "type": "bogus", "params": {} });
        let res = execute_crud("create", &args, &mut doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("failed to parse bogus"));
        assert_eq!(doc.nodes.len(), 0);
    }

    #[test]
    fn read_empty_document_returns_message() {
        let doc = empty_doc();
        let args = json!({});
        let mut mutable = doc.clone();
        let res = execute_crud("read", &args, &mut mutable);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.result.contains("empty"));
    }

    #[test]
    fn read_lists_parts_after_create() {
        let mut doc = empty_doc();
        let a = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}},"name":"A"}),
            &mut doc,
        );
        assert_eq!(a.status, ExecutionStatus::Success, "create A: {:?}", a);
        let b = execute_crud(
            "create",
            &json!({"type":"sphere","params":{"radius":5,"segments":0}, "name":"B"}),
            &mut doc,
        );
        assert_eq!(b.status, ExecutionStatus::Success, "create B: {:?}", b);
        let res = execute_crud("read", &json!({}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.result.contains("2 parts"), "got: {}", res.result);
        assert!(res.result.contains("\"A\""));
        assert!(res.result.contains("\"B\""));
    }

    #[test]
    fn read_by_part_id_returns_detail() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}},"name":"Target"}),
            &mut doc,
        );
        let part_id = created.part_id.expect("create returns part_id");
        let res = execute_crud("read", &json!({"part_id": part_id}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.result.contains("Target"));
        assert!(res.result.contains("cube"));
    }

    #[test]
    fn delete_removes_part() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":1,"y":1,"z":1}}}),
            &mut doc,
        );
        let id = created.part_id.unwrap();
        assert_eq!(doc.roots.len(), 1);
        let res = execute_crud("delete", &json!({"part_id": id}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert_eq!(doc.roots.len(), 0);
        assert_eq!(doc.nodes.len(), 0);
    }

    #[test]
    fn delete_nonexistent_is_error() {
        let mut doc = empty_doc();
        let res = execute_crud("delete", &json!({"part_id": "999"}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("part not found"));
    }

    #[test]
    fn set_material_updates_entry() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":1,"y":1,"z":1}}}),
            &mut doc,
        );
        let id = created.part_id.unwrap();
        let res = execute_crud(
            "set_material",
            &json!({"part_id": id, "material": "aluminum"}),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Success);
        assert_eq!(doc.roots[0].material, "aluminum");
    }

    #[test]
    fn update_merges_params_on_existing_node() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}},"name":"C"}),
            &mut doc,
        );
        assert_eq!(created.status, ExecutionStatus::Success);
        let node_id = created.node_id.expect("create returns node_id");

        let res = execute_crud(
            "update",
            &json!({
                "node_id": node_id,
                "params": { "size": { "x": 30, "y": 20, "z": 5 } }
            }),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Success, "got: {:?}", res);

        // Verify the new size landed on the live node.
        let nid: NodeId = node_id.parse().unwrap();
        let node = doc.nodes.get(&nid).unwrap();
        match &node.op {
            CsgOp::Cube { size } => {
                assert_eq!(size.x, 30.0);
                assert_eq!(size.y, 20.0);
                assert_eq!(size.z, 5.0);
            }
            other => panic!("expected Cube, got {other:?}"),
        }
    }

    #[test]
    fn update_nonexistent_is_error() {
        let mut doc = empty_doc();
        let res = execute_crud(
            "update",
            &json!({"node_id":"999","params":{"size":{"x":1,"y":1,"z":1}}}),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("node not found"));
    }

    #[test]
    fn update_rejects_incompatible_field() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}}}),
            &mut doc,
        );
        let node_id = created.node_id.unwrap();

        // `radius` isn't a field on Cube — serde should reject the
        // round-trip after we can't rebuild a Cube with a stray field.
        // (serde is permissive by default and ignores unknown fields, so
        // the field is dropped and the op stays valid — this asserts the
        // permissive behavior, which is what we want. Incompatible types
        // on an existing field would fail.)
        let res = execute_crud(
            "update",
            &json!({"node_id": node_id, "params": { "radius": 5 }}),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Success);

        // But a wrong-typed existing field should fail.
        let res = execute_crud(
            "update",
            &json!({"node_id": node_id, "params": { "size": "huge" }}),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Error);
    }

    #[test]
    fn update_cannot_change_variant_type() {
        let mut doc = empty_doc();
        let created = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}}}),
            &mut doc,
        );
        let node_id = created.node_id.unwrap();

        // Attempting to flip the variant via `type` should be ignored —
        // the node stays a Cube regardless of what the model sends.
        let res = execute_crud(
            "update",
            &json!({
                "node_id": node_id,
                "params": { "type": "Sphere", "radius": 5 }
            }),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Success);
        let nid: NodeId = node_id.parse().unwrap();
        let node = doc.nodes.get(&nid).unwrap();
        assert!(
            matches!(node.op, CsgOp::Cube { .. }),
            "variant must not change"
        );
    }

    #[test]
    fn unknown_tool_returns_error() {
        let mut doc = empty_doc();
        let res = execute_crud("frobnicate", &json!({}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("unknown tool"));
    }

    // -- plan_crud tests: verify the pure planner returns the right
    //    ToolOutcome shape without mutating the document.

    #[test]
    fn plan_create_returns_add_feature_outcome() {
        let doc = empty_doc();
        let args = json!({
            "type": "cube",
            "params": { "size": { "x": 10, "y": 10, "z": 10 } },
            "name": "PlanCube"
        });
        let res = plan_crud("create", &args, &doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.result.contains("Created cube"));
        let outcome = res.outcome.expect("create should produce an outcome");
        match outcome {
            ToolOutcome::AddFeature { op, name, parent_part_id } => {
                assert!(matches!(op, CsgOp::Cube { .. }));
                assert_eq!(name.as_deref(), Some("PlanCube"));
                assert_eq!(parent_part_id, None);
            }
            other => panic!("expected AddFeature, got {other:?}"),
        }
        // Planner must not mutate the document.
        assert_eq!(doc.nodes.len(), 0);
        assert_eq!(doc.roots.len(), 0);
    }

    #[test]
    fn plan_delete_validates_part_exists() {
        let doc = empty_doc();
        let res = plan_crud("delete", &json!({"part_id": "999"}), &doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.outcome.is_none());
    }

    #[test]
    fn plan_read_has_no_outcome() {
        let doc = empty_doc();
        let res = plan_crud("read", &json!({}), &doc);
        assert_eq!(res.status, ExecutionStatus::Success);
        assert!(res.outcome.is_none());
    }

    #[test]
    fn apply_outcome_mutates_doc() {
        let mut doc = empty_doc();
        let op = CsgOp::Cube {
            size: vcad_ir::Vec3::new(5.0, 5.0, 5.0),
        };
        let outcome = ToolOutcome::AddFeature {
            op,
            name: Some("direct".into()),
            parent_part_id: None,
        };
        let ids = apply_outcome(&mut doc, &outcome).unwrap();
        assert!(ids.node_id.is_some());
        assert_eq!(doc.nodes.len(), 1);
        assert_eq!(doc.roots.len(), 1);
    }

    // -- Variant coverage: exhaustively verify every CsgOp shape the
    //    Rust planner is expected to handle end-to-end via the TUI's
    //    `plan_crud → apply_outcome` pipeline. This is the source of
    //    truth for "what the TUI chat tool actually supports today" and
    //    the shopping list for FeatureInput parity on the web path.

    /// Run create → verify success → verify op variant matches.
    fn create_and_verify<F>(doc: &mut Document, args: Value, check: F)
    where
        F: Fn(&CsgOp),
    {
        let res = execute_crud("create", &args, doc);
        assert_eq!(
            res.status,
            ExecutionStatus::Success,
            "{args} failed: {}",
            res.result
        );
        let nid: NodeId = res
            .node_id
            .as_deref()
            .expect("node_id")
            .parse()
            .expect("parse nid");
        let node = doc.nodes.get(&nid).expect("node present");
        check(&node.op);
    }

    #[test]
    fn all_primitive_variants_plan_and_apply() {
        let mut doc = empty_doc();
        create_and_verify(
            &mut doc,
            json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}}}),
            |op| assert!(matches!(op, CsgOp::Cube { .. })),
        );
        create_and_verify(
            &mut doc,
            json!({"type":"cylinder","params":{"radius":5,"height":10,"segments":0}}),
            |op| assert!(matches!(op, CsgOp::Cylinder { .. })),
        );
        create_and_verify(
            &mut doc,
            json!({"type":"sphere","params":{"radius":5,"segments":0}}),
            |op| assert!(matches!(op, CsgOp::Sphere { .. })),
        );
        create_and_verify(
            &mut doc,
            json!({"type":"cone","params":{"radius_bottom":5,"radius_top":0,"height":10,"segments":0}}),
            |op| assert!(matches!(op, CsgOp::Cone { .. })),
        );
        assert_eq!(doc.roots.len(), 4);
    }

    #[test]
    fn boolean_variants_plan_and_apply() {
        let mut doc = empty_doc();
        // Seed two source parts to reference by id.
        let a = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}}}),
            &mut doc,
        )
        .node_id
        .unwrap();
        let b = execute_crud(
            "create",
            &json!({"type":"sphere","params":{"radius":6,"segments":0}}),
            &mut doc,
        )
        .node_id
        .unwrap();

        let left: NodeId = a.parse().unwrap();
        let right: NodeId = b.parse().unwrap();

        for kind in ["union", "difference", "intersection"] {
            let res = execute_crud(
                "create",
                &json!({
                    "type": kind,
                    "params": { "left": left, "right": right }
                }),
                &mut doc,
            );
            assert_eq!(
                res.status,
                ExecutionStatus::Success,
                "{kind} failed: {}",
                res.result
            );
            let nid: NodeId = res.node_id.unwrap().parse().unwrap();
            let node = doc.nodes.get(&nid).unwrap();
            match (kind, &node.op) {
                ("union", CsgOp::Union { .. })
                | ("difference", CsgOp::Difference { .. })
                | ("intersection", CsgOp::Intersection { .. }) => {}
                (k, op) => panic!("expected {k} variant, got {op:?}"),
            }
        }
    }

    #[test]
    fn transform_variants_plan_and_apply() {
        let mut doc = empty_doc();
        let child = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":10,"y":10,"z":10}}}),
            &mut doc,
        )
        .node_id
        .unwrap();
        let child_nid: NodeId = child.parse().unwrap();

        let cases = [
            (
                "translate",
                json!({"type":"translate","params":{"child": child_nid, "offset":{"x":5,"y":0,"z":0}}}),
            ),
            (
                "rotate",
                json!({"type":"rotate","params":{"child": child_nid, "angles":{"x":0,"y":0,"z":90}}}),
            ),
            (
                "scale",
                json!({"type":"scale","params":{"child": child_nid, "factor":{"x":2,"y":2,"z":2}}}),
            ),
        ];
        for (label, args) in cases {
            let res = execute_crud("create", &args, &mut doc);
            assert_eq!(
                res.status,
                ExecutionStatus::Success,
                "{label} failed: {}",
                res.result
            );
            let nid: NodeId = res.node_id.unwrap().parse().unwrap();
            let op = &doc.nodes.get(&nid).unwrap().op;
            match (label, op) {
                ("translate", CsgOp::Translate { .. })
                | ("rotate", CsgOp::Rotate { .. })
                | ("scale", CsgOp::Scale { .. }) => {}
                (l, op) => panic!("expected {l} variant, got {op:?}"),
            }
        }
    }

    #[test]
    fn modifier_variants_with_parent_rewire_roots() {
        let mut doc = empty_doc();
        let parent = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":20,"y":20,"z":20}}}),
            &mut doc,
        )
        .node_id
        .unwrap();
        let parent_nid: NodeId = parent.parse().unwrap();

        // Fillet wraps the parent and rewires the scene root.
        let res = execute_crud(
            "create",
            &json!({
                "type": "fillet",
                "params": { "child": parent_nid, "radius": 2.0 },
                "parent_part_id": parent
            }),
            &mut doc,
        );
        assert_eq!(res.status, ExecutionStatus::Success, "fillet: {}", res.result);
        let fillet_nid: NodeId = res.node_id.unwrap().parse().unwrap();
        // Scene root should now be the fillet, not the cube.
        assert_eq!(doc.roots.len(), 1);
        assert_eq!(doc.roots[0].root, fillet_nid);
        // Both nodes live in the document.
        assert!(doc.nodes.contains_key(&parent_nid));
        assert!(doc.nodes.contains_key(&fillet_nid));
    }

    #[test]
    fn pattern_variants_plan_and_apply() {
        let mut doc = empty_doc();
        let child = execute_crud(
            "create",
            &json!({"type":"cube","params":{"size":{"x":5,"y":5,"z":5}}}),
            &mut doc,
        )
        .node_id
        .unwrap();
        let child_nid: NodeId = child.parse().unwrap();

        let lp = execute_crud(
            "create",
            &json!({
                "type": "linear_pattern",
                "params": {
                    "child": child_nid,
                    "direction": {"x": 1, "y": 0, "z": 0},
                    "count": 3,
                    "spacing": 10.0
                }
            }),
            &mut doc,
        );
        assert_eq!(lp.status, ExecutionStatus::Success, "linear: {}", lp.result);

        let cp = execute_crud(
            "create",
            &json!({
                "type": "circular_pattern",
                "params": {
                    "child": child_nid,
                    "axis_origin": {"x": 0, "y": 0, "z": 0},
                    "axis_dir": {"x": 0, "y": 0, "z": 1},
                    "count": 6,
                    "angle_deg": 360.0
                }
            }),
            &mut doc,
        );
        assert_eq!(cp.status, ExecutionStatus::Success, "circular: {}", cp.result);
    }
}
