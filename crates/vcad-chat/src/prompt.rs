//! System prompt builder — Rust port of `CommandRegistry.buildSystemPrompt`.
//!
//! Mirrors `packages/core/src/commands/registry.ts:156-285`. The web reference
//! is the canonical version; any divergence here should be considered a bug.
//!
//! The prompt has four sections, concatenated:
//!
//! 1. **Preamble** — static workflow guide, sketch rules, material list.
//! 2. **Type catalog** — generated from [`crate::schemas::all_schemas`],
//!    one entry per `CsgOp` variant with description, ai_hint, and parameters.
//! 3. **Current document** — a flat listing of parts and their feature-tree
//!    nodes, built from a caller-supplied `&[PartInfo]`.
//! 4. **Selection** — the currently selected geometry, if any.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::schemas::all_schemas;
use vcad_ir::Document;

/// A single part in the caller's current document. Callers (TUI, later web
/// frontends) walk their `Document` and populate this. Keeping it decoupled
/// from `vcad_ir::Document` means the prompt builder stays trivially testable.
///
/// Serde is set up so the JSON wire format matches what the TS
/// `getDocumentParts()` helper already emits from the web document store:
/// camelCase fields, and `nodes` is optional (defaults to `[]`) since the
/// current web caller doesn't thread feature-tree nodes through. Rust
/// callers still use the snake_case field names natively — only the
/// on-the-wire representation is TS-shaped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default)]
    pub nodes: Vec<NodeInfo>,
}

/// A single feature-tree node belonging to a [`PartInfo`]. The
/// serde rename aligns with the TS `{ nodeId, type, params }` shape
/// that `getDocumentParts` would produce if it ever grew nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    /// Serialized param object. Typically a JSON object; any non-object
    /// value is rendered via `serde_json::to_string`.
    pub params: Value,
}

/// Selected geometry entry corresponding to the web app's
/// `SelectionContext` shape in `packages/core/src/stores/chat-store.ts`.
/// camelCase on the wire; the Rust side stays snake_case.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionInfo {
    pub part_id: String,
    pub part_name: String,
    pub geometry_type: String,
}

/// Walk a [`vcad_ir::Document`] and produce the [`PartInfo`] view the
/// prompt builder needs. Shared helper so every frontend (TUI today,
/// WASM-bridged web next, future Dioxus/Blitz native after that)
/// computes parts identically.
pub fn parts_from_document(doc: &Document) -> Vec<PartInfo> {
    doc.roots
        .iter()
        .filter_map(|entry| {
            let node = doc.nodes.get(&entry.root)?;
            let op_value = serde_json::to_value(&node.op).ok()?;
            let kind = op_value
                .get("type")
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .unwrap_or_else(|| "csg_op".to_string());
            let params = op_value
                .as_object()
                .map(|o| {
                    let mut clone = o.clone();
                    clone.remove("type");
                    Value::Object(clone)
                })
                .unwrap_or(Value::Null);
            Some(PartInfo {
                id: entry.root.to_string(),
                name: node
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("part {}", entry.root)),
                kind: kind.clone(),
                nodes: vec![NodeInfo {
                    node_id: entry.root.to_string(),
                    node_type: kind,
                    params,
                }],
            })
        })
        .collect()
}

/// Build the full system prompt — preamble, type catalog, doc state, selection.
pub fn build_system_prompt(parts: &[PartInfo], selection: &[SelectionInfo]) -> String {
    let mut out = String::with_capacity(8192);

    out.push_str(PREAMBLE);
    out.push_str("\n\n");
    out.push_str(&type_catalog());

    if !parts.is_empty() {
        out.push_str("\n## Current Document\nParts:\n");
        for part in parts {
            out.push_str(&format!("- {} \"{}\" [{}]\n", part.id, part.name, part.kind));
            for node in &part.nodes {
                let params_str = format_params(&node.params);
                out.push_str(&format!(
                    "  └─ {} [{}] {}\n",
                    node.node_id, node.node_type, params_str
                ));
            }
        }
        out.push('\n');
    }

    if !selection.is_empty() {
        out.push_str("Selected:\n");
        for sel in selection {
            out.push_str(&format!(
                "- {} ({}, id: {})\n",
                sel.part_name, sel.geometry_type, sel.part_id
            ));
        }
    }

    out
}

/// Generate the "## Type Catalog" section from the schema list. Mirrors
/// `getTypeCatalog` in `registry.ts:130-153`.
pub fn type_catalog() -> String {
    let mut out = String::with_capacity(4096);
    out.push_str("## Type Catalog\n\n");
    for schema in all_schemas() {
        out.push_str(&format!("### {} ({})\n", schema.name, schema.category));
        out.push_str(&schema.description);
        out.push('\n');
        if let Some(hint) = &schema.ai_hint {
            out.push_str(hint);
            out.push('\n');
        }
        if let Some(props) = schema
            .input_schema
            .get("properties")
            .and_then(|p| p.as_object())
        {
            if !props.is_empty() {
                out.push_str("Parameters:\n");
                for (key, prop) in props {
                    let ty = prop
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("object");
                    let desc = prop
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    if desc.is_empty() {
                        out.push_str(&format!("- {key}: {ty}\n"));
                    } else {
                        out.push_str(&format!("- {key}: {ty} — {desc}\n"));
                    }
                }
            }
        }
        out.push('\n');
    }
    out
}

/// Render a param `Value` as `k1: v1, k2: v2` — mirrors the web's
/// `Object.entries(params).map(([k,v]) => ...).join(", ")` formatter.
fn format_params(params: &Value) -> String {
    match params {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}: {}", serde_json::to_string(v).unwrap_or_default()))
            .collect::<Vec<_>>()
            .join(", "),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Preamble — the static workflow guide. Kept in lockstep with
// `registry.ts:161-255`. If the web copy changes, update this constant and
// bump the test that pins section headings.
// ---------------------------------------------------------------------------

const PREAMBLE: &str = r#"You are vcad's AI assistant — a parametric CAD copilot.
Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.
You have four tools: create, read, update, delete.

## Workflow Guide

**Creating shapes:** Use create with type and params. For custom profiles, use extrude with an inline sketch.
**Modifying:** Use update with a node_id and the params to change. Use read to find node IDs.
**Modifiers:** Fillet, chamfer, shell — pass parent_part_id to apply to an existing part.
**Booleans:** Union, difference, intersection — pass left and right part IDs, or let the user select 2 parts.
**Patterns:** Linear and circular patterns repeat geometry. Pass child (part ID), count, spacing/angle.
**Transforms:** Translate, rotate, scale — pass child (part ID) and the transform values.

## Design Patterns

1. **Simple shapes:** create cube/cylinder/sphere with size/radius params
2. **Custom profiles:** create extrude with inline sketch (Line and Arc segments forming a closed loop)
3. **Refinement:** create fillet/chamfer with parent_part_id after creating base geometry
4. **Hollowing:** create shell with parent_part_id for containers/enclosures
5. **Combining:** create difference to cut holes, union to join, intersection to keep overlap
6. **Repetition:** create linear_pattern or circular_pattern for bolt holes, fins, etc.
7. **Assemblies:** Create multiple parts, position with translate/rotate

## Available Materials

Use set_material(part_id, material) to color parts. Preset keys:
- **Metals**: aluminum, steel, brass, copper, titanium, chrome, gold, silver
- **Plastics**: abs-white, abs-black, abs-red, abs-blue, pla, petg, nylon, resin, acrylic, rubber
- **Organic**: oak, walnut, leather, cork, bamboo
- **Glass**: glass, glass-tinted
- **Composite**: carbon-fiber, fiberglass, kevlar
- **Other**: concrete, ceramic, foam

For the Sun, use gold. For Mars, copper. For Earth, glass-tinted. For gold/silver metallic parts use gold/silver. There's no pure "color" system — you pick the closest preset.

## Orientation Notes

- **Cylinder**: axis along Z. Height is along Z, circular face is in XY plane. To make a wheel (round face visible from the side), use extrude with a circular sketch in the XZ plane (set y_dir to (0,0,1)) and direction along Y for thickness — this is more reliable than rotating a cylinder.
- **Box/Cube**: size.x is width, size.y is depth, size.z is height.
- The grid lies in the XY plane. Z is up. X is right, Y is forward.

## How translate/rotate/scale work (IMPORTANT)

When you call create with type translate/rotate/scale, it WRAPS the existing part in place. The returned id is the SAME as the input child id — it does NOT create a new part with a new id. So:
- create(type:translate, params:{child:abc, offset:...}) → part abc still exists, now translated. Use abc for follow-ups.
- The DAG node behind the part gets a new wrapping node, but you reference parts by their stable part id, not by node id.

## Key Rules

- Take as many tool calls as you need — there's no fixed cap. The user can click Stop to interrupt. Plan efficiently and stop naturally when the request is fulfilled; don't keep making changes for the sake of it.
- **CRITICAL — part IDs**: Every create tool returns a part id in the result (e.g. "Created cylinder with id: 1775951370705:0"). You MUST use the EXACT id string from the result in follow-up operations. NEVER invent ids like "part_1" or "part_2" — those will fail validation.
- **ALWAYS name features** with a short descriptive name param on every create call. Good names: "Front Wheel", "Top Tube", "Seat Post", "Left Eye". Bad names: (empty), "part1", "thing". Names make the feature tree readable and chat summaries meaningful.
- **CRITICAL — closed sketches**: Sketch segments MUST form a CLOSED loop where each segment's end point matches the next segment's start point, and the last segment's end matches the first segment's start. A single arc is NOT a closed loop. For a crescent/smile shape, use TWO arcs (outer + inner curve) connected by lines at the endpoints.
- Vec3 params are {x, y, z} objects. Angles are in degrees. Units are mm.
- Be concise — briefly confirm what you did after tool calls.
- When the user says "this" or "it", use the selected geometry context.
- For complex models with many parts, create the most important features first, then offer to continue.
- NEVER use emojis unless the user explicitly requests them.

## Closed Sketch Example (for reference)

A rectangle (20×15 mm) as a closed loop:
~~~
segments: [
  {type:"Line", start:{x:0,y:0},   end:{x:20,y:0}},
  {type:"Line", start:{x:20,y:0},  end:{x:20,y:15}},
  {type:"Line", start:{x:20,y:15}, end:{x:0,y:15}},
  {type:"Line", start:{x:0,y:15},  end:{x:0,y:0}}
]
~~~

A crescent (smile) as a closed loop with two arcs:
~~~
segments: [
  {type:"Arc", start:{x:-25,y:0}, end:{x:25,y:0}, center:{x:0,y:-30}, ccw:false},
  {type:"Arc", start:{x:25,y:0},  end:{x:-25,y:0}, center:{x:0,y:-20}, ccw:true}
]
~~~

A full circle of radius R using two half-arcs:
~~~
segments: [
  {type:"Arc", start:{x:R,y:0},  end:{x:-R,y:0}, center:{x:0,y:0}, ccw:false},
  {type:"Arc", start:{x:-R,y:0}, end:{x:R,y:0},  center:{x:0,y:0}, ccw:false}
]
~~~

## Sketch rules — READ CAREFULLY

- Sketches MUST have at least 2 segments forming a closed loop.
- Each segment's end must EXACTLY match the next segment's start.
- The LAST segment's end must match the FIRST segment's start.
- **Arcs**: start, end, center and ccw. The radius = distance(start, center) must equal distance(end, center) — if they don't match, the arc is invalid. For a full circle use TWO half-arcs (e.g. start=(r,0), end=(-r,0), center=(0,0), ccw=false, then start=(-r,0), end=(r,0), center=(0,0), ccw=false).
- To cut a shape OUT of a part, create the shape as a separate extrude, then use difference(left: mainPartId, right: cutPartId).
- Don't use single arcs — they aren't closed profiles."#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_prompt_has_preamble_and_catalog() {
        let prompt = build_system_prompt(&[], &[]);
        assert!(prompt.contains("You are vcad's AI assistant"));
        assert!(prompt.contains("## Type Catalog"));
        // No doc-state section when parts list is empty.
        assert!(!prompt.contains("## Current Document"));
        assert!(!prompt.contains("Selected:"));
    }

    #[test]
    fn parts_render_as_tree() {
        let parts = vec![PartInfo {
            id: "1:0".to_string(),
            name: "Top Tube".to_string(),
            kind: "primitive".to_string(),
            nodes: vec![NodeInfo {
                node_id: "1:0".to_string(),
                node_type: "cube".to_string(),
                params: json!({"size": {"x": 20, "y": 5, "z": 5}}),
            }],
        }];
        let prompt = build_system_prompt(&parts, &[]);
        assert!(prompt.contains("## Current Document"));
        assert!(prompt.contains("1:0 \"Top Tube\" [primitive]"));
        assert!(prompt.contains("└─ 1:0 [cube]"));
    }

    #[test]
    fn selection_rendered_when_present() {
        let selection = vec![SelectionInfo {
            part_id: "abc".to_string(),
            part_name: "Wheel".to_string(),
            geometry_type: "cylinder".to_string(),
        }];
        let prompt = build_system_prompt(&[], &selection);
        assert!(prompt.contains("Selected:"));
        assert!(prompt.contains("Wheel (cylinder, id: abc)"));
    }

    #[test]
    fn type_catalog_includes_core_variants() {
        let catalog = type_catalog();
        assert!(catalog.contains("### cube"));
        assert!(catalog.contains("### cylinder"));
        assert!(catalog.contains("### sphere"));
        assert!(catalog.contains("Parameters:"));
    }

    #[test]
    fn format_params_renders_object_keys() {
        let v = json!({"x": 1, "y": 2});
        let s = format_params(&v);
        assert!(s.contains("x: 1"));
        assert!(s.contains("y: 2"));
    }
}
