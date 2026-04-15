//! CRUD tool executor — Rust port of `executeCrud` from
//! `packages/core/src/commands/executors.ts`.
//!
//! Takes a `&mut vcad_ir::Document` and a tool name + args, returns a
//! structured [`ExecutionResult`] that the chat panel can render. Unlike
//! the TS version — which wraps a huge Zustand `DocumentStore` — the Rust
//! version mutates the IR document directly. A future frontend that needs
//! different semantics (e.g. CRDT-backed, undo-aware) can wrap the same
//! functions with a trait layer.
//!
//! Scope for M3.5:
//! - **create**       — full support via `CsgOp` serde roundtrip.
//! - **read**         — list parts or describe one by part id.
//! - **delete**       — removes the scene entry and its root node.
//! - **set_material** — updates the scene entry's material key.
//! - **update**       — stub; real per-variant param merging is M3.5+.

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

/// Dispatch a CRUD tool call. `tool` is one of `create` / `read` / `update` /
/// `delete` / `set_material`; `args` is the Anthropic-supplied input object.
pub fn execute_crud(tool: &str, args: &Value, doc: &mut Document) -> ExecutionResult {
    match tool {
        "create" => execute_create(args, doc),
        "read" => execute_read(args, doc),
        "update" => execute_update(args, doc),
        "delete" => execute_delete(args, doc),
        "set_material" => execute_set_material(args, doc),
        other => ExecutionResult::error(format!("unknown tool: {other}")),
    }
}

// ---------------------------------------------------------------------------
// create
// ---------------------------------------------------------------------------

fn execute_create(args: &Value, doc: &mut Document) -> ExecutionResult {
    let Some(ty) = args.get("type").and_then(|v| v.as_str()) else {
        return ExecutionResult::error("create requires `type`");
    };
    let params = args
        .get("params")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let name = args.get("name").and_then(|v| v.as_str()).map(str::to_string);
    let parent_part_id = args.get("parent_part_id").and_then(|v| v.as_str());

    // CsgOp carries `#[serde(tag = "type")]` with no rename_all, so serde
    // wants the PascalCase variant name ("Cube") — but the ToolSchema proc
    // macro publishes snake_case names ("cube"). Bridge the two: map
    // snake_case → PascalCase via the schema list (authoritative) before
    // handing off to serde. Unknown names fall through as-is and serde
    // will surface a clear error.
    let pascal_ty = snake_to_pascal(ty);

    let mut obj = Map::new();
    obj.insert("type".into(), Value::String(pascal_ty));
    if let Value::Object(p) = params {
        for (k, v) in p {
            obj.insert(k, v);
        }
    } else {
        return ExecutionResult::error("create `params` must be an object");
    }

    let op: CsgOp = match serde_json::from_value(Value::Object(obj)) {
        Ok(op) => op,
        Err(e) => return ExecutionResult::error(format!("failed to parse {ty}: {e}")),
    };

    let new_id = next_node_id(doc);
    doc.nodes.insert(
        new_id,
        Node {
            id: new_id,
            name,
            op,
        },
    );

    if let Some(parent) = parent_part_id {
        // Rewire the parent scene entry so it points at the new node. The
        // caller is responsible for having the new node reference the
        // previous root through its own fields (fillet/shell/etc. have a
        // `parent` field in their params).
        let Ok(parent_nid) = parent.parse::<NodeId>() else {
            return ExecutionResult::error(format!("invalid parent_part_id: {parent}"));
        };
        let Some(entry) = doc.roots.iter_mut().find(|e| e.root == parent_nid) else {
            return ExecutionResult::error(format!("parent part not found: {parent}"));
        };
        entry.root = new_id;
        ExecutionResult::success(format!("Wrapped part {parent} with {ty}, new id: {new_id}"))
            .with_part(new_id.to_string())
            .with_node(new_id.to_string())
    } else {
        doc.roots.push(SceneEntry {
            root: new_id,
            material: "default".to_string(),
            visible: None,
        });
        ExecutionResult::success(format!("Created {ty} with id: {new_id}"))
            .with_part(new_id.to_string())
            .with_node(new_id.to_string())
    }
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
// read
// ---------------------------------------------------------------------------

fn execute_read(args: &Value, doc: &Document) -> ExecutionResult {
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
// update — stub; real per-variant merging lands in a follow-up
// ---------------------------------------------------------------------------

fn execute_update(_args: &Value, _doc: &mut Document) -> ExecutionResult {
    ExecutionResult::error(
        "update is not yet implemented in the Rust executor — delete and \
         recreate the node, or update the document directly for now.",
    )
}

// ---------------------------------------------------------------------------
// delete
// ---------------------------------------------------------------------------

fn execute_delete(args: &Value, doc: &mut Document) -> ExecutionResult {
    let Some(part_id) = args.get("part_id").and_then(|v| v.as_str()) else {
        return ExecutionResult::error("delete requires `part_id`");
    };
    let Ok(nid) = part_id.parse::<NodeId>() else {
        return ExecutionResult::error(format!("invalid part_id: {part_id}"));
    };
    let before = doc.roots.len();
    doc.roots.retain(|e| e.root != nid);
    if doc.roots.len() == before {
        return ExecutionResult::error(format!("part not found: {part_id}"));
    }
    doc.nodes.remove(&nid);
    ExecutionResult::success(format!("Deleted part {part_id}"))
}

// ---------------------------------------------------------------------------
// set_material
// ---------------------------------------------------------------------------

fn execute_set_material(args: &Value, doc: &mut Document) -> ExecutionResult {
    let Some(part_id) = args.get("part_id").and_then(|v| v.as_str()) else {
        return ExecutionResult::error("set_material requires `part_id`");
    };
    let Some(material) = args.get("material").and_then(|v| v.as_str()) else {
        return ExecutionResult::error("set_material requires `material`");
    };
    let Ok(nid) = part_id.parse::<NodeId>() else {
        return ExecutionResult::error(format!("invalid part_id: {part_id}"));
    };
    let Some(entry) = doc.roots.iter_mut().find(|e| e.root == nid) else {
        return ExecutionResult::error(format!("part not found: {part_id}"));
    };
    entry.material = material.to_string();
    ExecutionResult::success(format!("Set {part_id} material to {material}"))
        .with_part(part_id.to_string())
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
    fn update_returns_not_implemented() {
        let mut doc = empty_doc();
        let res = execute_crud("update", &json!({"node_id":"1","params":{}}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("not yet implemented"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let mut doc = empty_doc();
        let res = execute_crud("frobnicate", &json!({}), &mut doc);
        assert_eq!(res.status, ExecutionStatus::Error);
        assert!(res.result.contains("unknown tool"));
    }
}
