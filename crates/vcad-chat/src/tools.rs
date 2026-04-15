//! Anthropic tool definitions — Rust port of `CommandRegistry.toAnthropicTools`.
//!
//! Mirrors `packages/core/src/commands/registry.ts:26-127`. The five CRUD
//! tools are:
//!
//! - `create`       — create a new part or feature from a CsgOp variant
//! - `read`         — inspect parts / features (list or detail)
//! - `update`       — update params on an existing node
//! - `delete`       — delete a part
//! - `set_material` — assign a material preset to a part

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::schemas::type_enum;

/// An Anthropic tool definition — the exact shape accepted by the
/// Anthropic Messages API under `tools`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnthropicTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Build the canonical five-tool set. Matches the web app's
/// `commandRegistry.toAnthropicTools()` byte-for-byte so the `/api/chat`
/// endpoint sees the same payload from either client.
pub fn anthropic_tools() -> Vec<AnthropicTool> {
    let type_values: Vec<Value> = type_enum().into_iter().map(Value::String).collect();

    vec![
        AnthropicTool {
            name: "create".to_string(),
            description:
                "Create a new CAD feature. Use 'type' to specify the kind and 'params' for its \
                 parameters. See the type catalog in the system prompt for available types and \
                 their parameters."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "type": {
                        "type": "string",
                        "enum": type_values,
                        "description": "The CsgOp type to create."
                    },
                    "params": {
                        "type": "object",
                        "description": "Parameters for the specified type. See type catalog in system prompt."
                    },
                    "parent_part_id": {
                        "type": "string",
                        "description": "If provided, appends the feature to this existing part instead of creating a new one."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional human-readable name for this feature (e.g. 'Front Wheel', 'Top Tube', 'Seat Post'). Strongly recommended — makes the feature tree readable. Keep short (1-3 words)."
                    }
                },
                "required": ["type", "params"]
            }),
        },
        AnthropicTool {
            name: "read".to_string(),
            description:
                "Inspect the current document. Without part_id, lists all parts. With part_id, \
                 returns full feature tree and parameters for that part."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "part_id": {
                        "type": "string",
                        "description": "Part ID to inspect. Omit to list all parts."
                    }
                }
            }),
        },
        AnthropicTool {
            name: "update".to_string(),
            description:
                "Update parameters on an existing node. Pass only the fields you want to change."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "node_id": {
                        "type": "string",
                        "description": "The node ID to update."
                    },
                    "params": {
                        "type": "object",
                        "description": "Partial parameter object. Only provided fields are changed."
                    }
                },
                "required": ["node_id", "params"]
            }),
        },
        AnthropicTool {
            name: "delete".to_string(),
            description: "Delete a part from the document.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "part_id": {
                        "type": "string",
                        "description": "The part ID to delete."
                    }
                },
                "required": ["part_id"]
            }),
        },
        AnthropicTool {
            name: "set_material".to_string(),
            description:
                "Set the material/color of a part. Use one of the preset material keys listed \
                 in the system prompt."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "part_id": {
                        "type": "string",
                        "description": "The part ID to set the material on."
                    },
                    "material": {
                        "type": "string",
                        "description": "Material preset key (e.g. 'aluminum', 'gold', 'oak', 'abs-red')."
                    }
                },
                "required": ["part_id", "material"]
            }),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_five_tools() {
        let tools = anthropic_tools();
        assert_eq!(tools.len(), 5);
        let names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["create", "read", "update", "delete", "set_material"]);
    }

    #[test]
    fn create_tool_enum_is_populated() {
        let tools = anthropic_tools();
        let create = tools.iter().find(|t| t.name == "create").unwrap();
        let type_enum = create
            .input_schema
            .get("properties")
            .and_then(|p| p.get("type"))
            .and_then(|t| t.get("enum"))
            .and_then(|e| e.as_array())
            .expect("create tool should have a type.enum array");
        assert!(!type_enum.is_empty());
        // Must contain the core primitives.
        let has_cube = type_enum
            .iter()
            .any(|v| v.as_str() == Some("cube"));
        assert!(has_cube, "create tool type enum should include 'cube'");
    }

    #[test]
    fn required_fields_match_web() {
        let tools = anthropic_tools();
        let find = |name: &str| -> &AnthropicTool {
            tools.iter().find(|t| t.name == name).unwrap()
        };
        let required = |t: &AnthropicTool| -> Vec<String> {
            t.input_schema
                .get("required")
                .and_then(|r| r.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(required(find("create")), vec!["type", "params"]);
        assert_eq!(required(find("update")), vec!["node_id", "params"]);
        assert_eq!(required(find("delete")), vec!["part_id"]);
        assert_eq!(required(find("set_material")), vec!["part_id", "material"]);
        assert!(required(find("read")).is_empty());
    }
}
