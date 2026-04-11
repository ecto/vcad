# CRUD Tool Registry Design

Kernel-driven command registry with CRUD tool API for vcad's AI chat system.

Supersedes the hand-wired tool approach. Builds on the unified command/chat design from 2026-04-08 but simplifies the tool surface to four CRUD verbs with schemas generated from the Rust IR.

## Goals

1. Single source of truth: CsgOp variants in Rust define what tools exist and their parameter schemas
2. Four-verb CRUD API: create, read, update, delete — replaces 8+ individual tools
3. System prompt awareness: AI sees full document state every turn without calling read first
4. Sketch/extrude support: compound and decomposed sketch workflows via create
5. Multi-consumer registry: chat middleware, command palette, and MCP server all consume the same registry

## Architecture Overview

```
┌──────────────────────────────────────────────────────────┐
│  crates/vcad-ir/src/lib.rs                               │
│  #[derive(ToolSchema)] on CsgOp enum                     │
│  Proc macro extracts JSON Schema + metadata from types   │
└──────────────┬───────────────────────────────────────────┘
               │ generates
               ▼
┌──────────────────────────────────────────────────────────┐
│  crates/vcad-kernel-wasm/src/lib.rs                      │
│  get_tool_schemas() → JSON string                        │
│  Exports schema array via WASM                           │
└──────────────┬───────────────────────────────────────────┘
               │ consumed by
               ▼
┌──────────────────────────────────────────────────────────┐
│  packages/core/src/commands/                             │
│  CommandRegistry — loads schemas, pairs with executors   │
│  Outputs: Anthropic tools, palette items, MCP tools      │
│  Builds system prompt with type catalog + document state │
└──────────────┬───────────────────────────────────────────┘
               │ used by
               ▼
┌────────────┐ ┌────────────┐ ┌────────────┐
│ Chat API   │ │ Cmd Palette│ │ MCP Server │
└────────────┘ └────────────┘ └────────────┘
```

## 1. Proc Macro: `vcad-tool-derive`

### Crate

New crate at `crates/vcad-tool-derive/` with `proc-macro = true`.

### Derive Macro

`#[derive(ToolSchema)]` on `CsgOp` generates `CsgOp::tool_schemas() -> Vec<ToolSchemaEntry>`.

### Attributes

- `#[tool(category = "...")]` — variant-level category override (default from enum-level attribute)
- `#[tool(ai_hint = "...")]` — extra context for the AI system prompt
- `#[tool(hidden)]` — skip this variant (not exposed as a tool)
- `#[tool(default = "...")]` — override the default value for a field

### Generated Output

```rust
pub struct ToolSchemaEntry {
    /// Variant name in snake_case (e.g. "cube", "extrude", "linear_pattern")
    pub name: String,
    /// From doc comment on the variant
    pub description: String,
    /// From #[tool(category)] attribute
    pub category: String,
    /// From #[tool(ai_hint)] attribute
    pub ai_hint: Option<String>,
    /// Standard JSON Schema for the variant's fields
    pub input_schema: serde_json::Value,
}
```

### Type Mapping (Rust → JSON Schema)

| Rust Type | JSON Schema |
|-----------|-------------|
| `f64` | `{ "type": "number" }` |
| `u32` | `{ "type": "integer" }` |
| `bool` | `{ "type": "boolean" }` |
| `String` | `{ "type": "string" }` |
| `Vec3` | `{ "type": "object", "properties": { "x": number, "y": number, "z": number }, "required": ["x","y","z"] }` |
| `Vec2` | `{ "type": "object", "properties": { "x": number, "y": number }, "required": ["x","y"] }` |
| `Option<T>` | schema of T, not in `required` array |
| `Vec<T>` | `{ "type": "array", "items": <schema of T> }` |
| `NodeId` | `{ "type": "string" }` (node reference) |
| Tagged enums (e.g. `SketchSegment2D`, `PathCurve`) | `{ "oneOf": [...] }` with discriminator |

### Field Descriptions

Doc comments on fields become `"description"` in the JSON Schema property.

### Hidden Variants

The following CsgOp variants get `#[tool(hidden)]`:
- `Empty` — internal identity element
- `ImportedMesh` — drag-drop only, not AI-creatable
- `StepImport` — file import, not AI-creatable
- `PcbBoard` — domain-specific, not general CAD
- `EmbroideryPattern` — domain-specific

Boolean ops (`Union`, `Difference`, `Intersection`) and transforms (`Translate`, `Rotate`, `Scale`) remain visible — the AI can create them as nodes.

### WASM Export

```rust
// crates/vcad-kernel-wasm/src/lib.rs
#[wasm_bindgen]
pub fn get_tool_schemas() -> String {
    serde_json::to_string(&CsgOp::tool_schemas()).unwrap()
}
```

## 2. CRUD Tool API

Four tools replace all existing hand-wired tools.

### `create`

Creates a new IR node. Returns the created part ID or node ID.

```json
{
  "name": "create",
  "description": "Create a new CAD feature. Use 'type' to specify the CsgOp variant and 'params' for its parameters.",
  "input_schema": {
    "type": "object",
    "properties": {
      "type": {
        "type": "string",
        "enum": ["cube", "cylinder", "sphere", "cone", "extrude", "revolve", "sweep", "loft", "fillet", "chamfer", "shell", "translate", "rotate", "scale", "linear_pattern", "circular_pattern", "sketch_2d", "text_2d", "union", "difference", "intersection"]
      },
      "params": {
        "type": "object",
        "description": "Parameters matching the selected type's schema. See type catalog in system prompt."
      },
      "parent_part_id": {
        "type": "string",
        "description": "If provided, appends the feature to an existing part instead of creating a new one."
      }
    },
    "required": ["type", "params"]
  }
}
```

The `type` enum and `params` validation are generated from the proc macro output. The executor dispatches to the appropriate document store method.

#### Inline Sketch Handling

For `extrude`, `revolve`, `sweep`, and `loft`, the `sketch` parameter accepts either:
- A **string** (node ID reference to an existing Sketch2D node)
- An **inline object** with `origin`, `x_dir`, `y_dir`, `segments` — the executor creates the Sketch2D node internally

```json
{
  "type": "extrude",
  "params": {
    "sketch": {
      "origin": {"x": 0, "y": 0, "z": 0},
      "x_dir": {"x": 1, "y": 0, "z": 0},
      "y_dir": {"x": 0, "y": 1, "z": 0},
      "segments": [
        {"type": "Line", "start": {"x": 0, "y": 0}, "end": {"x": 20, "y": 0}},
        {"type": "Line", "start": {"x": 20, "y": 0}, "end": {"x": 20, "y": 15}},
        {"type": "Line", "start": {"x": 20, "y": 15}, "end": {"x": 0, "y": 15}},
        {"type": "Line", "start": {"x": 0, "y": 15}, "end": {"x": 0, "y": 0}}
      ]
    },
    "direction": {"x": 0, "y": 0, "z": 10}
  }
}
```

### `read`

Inspects document state.

```json
{
  "name": "read",
  "description": "Inspect the current document. Without partId, lists all parts. With partId, returns full feature tree and parameters.",
  "input_schema": {
    "type": "object",
    "properties": {
      "part_id": {
        "type": "string",
        "description": "Part ID to inspect. Omit to list all parts."
      }
    }
  }
}
```

**List mode** (no partId) returns:
```json
[
  {"id": "part-1", "name": "Base Plate", "rootType": "cube", "nodeCount": 3},
  {"id": "part-2", "name": "Pin", "rootType": "cylinder", "nodeCount": 2}
]
```

**Inspect mode** (with partId) returns:
```json
{
  "id": "part-1",
  "name": "Base Plate",
  "nodes": [
    {"nodeId": "node-1", "type": "cube", "params": {"size": {"x": 100, "y": 80, "z": 10}}},
    {"nodeId": "node-3", "type": "fillet", "params": {"radius": 2}},
    {"nodeId": "node-4", "type": "shell", "params": {"thickness": 1.5}}
  ],
  "transform": {"translation": {"x": 0, "y": 0, "z": 0}, "rotation": {"x": 0, "y": 0, "z": 0}, "scale": {"x": 1, "y": 1, "z": 1}},
  "boundingBox": {"min": {"x": -50, "y": -40, "z": 0}, "max": {"x": 50, "y": 40, "z": 10}}
}
```

### `update`

Modifies parameters on an existing node.

```json
{
  "name": "update",
  "description": "Update parameters on an existing node. Pass only the fields you want to change.",
  "input_schema": {
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
  }
}
```

Example: `update("node-3", {"radius": 5})` changes a fillet's radius from 2 to 5.

The executor calls `setFeatureParam(partId, key, toCrdtValue(value))` for each field in `params`. It resolves the node's parent part internally.

### `delete`

Removes a part.

```json
{
  "name": "delete",
  "description": "Delete a part from the document.",
  "input_schema": {
    "type": "object",
    "properties": {
      "part_id": {
        "type": "string",
        "description": "The part ID to delete."
      }
    },
    "required": ["part_id"]
  }
}
```

## 3. System Prompt

Regenerated before every API call. Built by `registry.buildSystemPrompt(documentState, selection)`.

### Template

```
You are vcad's AI assistant — a parametric CAD copilot.
Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.

You have four tools: create, read, update, delete.

## Type Catalog
<for each non-hidden CsgOp variant>
### <name> (<category>)
<description>
<ai_hint if present>
Parameters:
- <field>: <type> — <description>
</for each>

## Current Document
Parts:
<for each part>
- <partId> "<name>" [<rootType>]
  <for each node in feature tree>
  └─ <nodeId> [<type>] <key>: <value>, ...
  </for each>
</for each>

Selected: <partId> (<geometryType> <index>)
```

### Token Budget

Typical document with 10 parts, 3 features each ≈ 800-1200 tokens for the document section. Type catalog ≈ 1500-2000 tokens (generated once, stable). Total system prompt ≈ 3000-3500 tokens — well within budget.

For very large documents (50+ parts), truncate to selected part + nearest neighbors and include a note to use `read` for full listing.

## 4. TypeScript Command Registry

### File Structure

```
packages/core/src/commands/
├── types.ts         — Command, ToolSchemaEntry, ExecutionResult interfaces
├── registry.ts      — CommandRegistry class
├── executors.ts     — CRUD executor implementations
├── system-prompt.ts — system prompt builder
└── index.ts         — barrel export
```

### Types

```typescript
/** Mirrors the Rust ToolSchemaEntry — parsed from WASM JSON at init */
interface ToolSchemaEntry {
  name: string;
  description: string;
  category: string;
  aiHint?: string;
  inputSchema: Record<string, unknown>; // JSON Schema object
}

interface ExecutionResult {
  status: "success" | "error";
  result: string;   // human-readable summary returned to AI
  partId?: string;   // if a part was created/modified
  nodeId?: string;   // if a node was created/modified
}
```

### CommandRegistry

```typescript
class CommandRegistry {
  private schemas: ToolSchemaEntry[];

  /** Load schemas from WASM at init */
  async init(wasmModule: WasmModule): Promise<void>;

  /** Get the four CRUD tool definitions in Anthropic format */
  toAnthropicTools(): AnthropicTool[];

  /** Get tool definitions in MCP format */
  toMcpTools(): McpTool[];

  /** Get command palette items (create types + read/update/delete) */
  toPaletteItems(): PaletteItem[];

  /** Execute a CRUD operation */
  execute(tool: string, args: Record<string, unknown>, store: DocumentStore): ExecutionResult;

  /** Build system prompt with type catalog + document state */
  buildSystemPrompt(state: DocumentState, selection?: SelectionContext[]): string;

  /** Get type catalog section (cacheable — only changes on schema reload) */
  getTypeCatalog(): string;
}
```

### Executor Dispatch

```typescript
// executors.ts
function executeCreate(type: string, params: Record<string, unknown>, parentPartId: string | undefined, store: DocumentStore): ExecutionResult;
function executeRead(partId: string | undefined, store: DocumentStore): ExecutionResult;
function executeUpdate(nodeId: string, params: Record<string, unknown>, store: DocumentStore): ExecutionResult;
function executeDelete(partId: string, store: DocumentStore): ExecutionResult;
```

`executeCreate` maps CsgOp type names to document store methods:
- Primitives (cube, cylinder, sphere, cone) → `store.addPrimitive(kind)` + `store.updatePrimitiveOp(partId, params)`
- Sketch ops (extrude, revolve, sweep, loft) → `store.addExtrude(...)`, `store.addRevolve(...)`, etc.
- Modifiers (fillet, chamfer, shell) → `store.addFillet(...)`, etc., applied to `parentPartId`
- Transforms (translate, rotate, scale) → `store.setTranslation(...)`, etc.
- Booleans (union, difference, intersection) → `store.applyBoolean(...)`
- Patterns (linear_pattern, circular_pattern) → via CRDT engine

`executeUpdate` resolves the node's CsgOp type, validates `params` against the schema, then calls `store.setFeatureParam(partId, key, toCrdtValue(value))` for each field.

## 5. Migration

### Removed

- Tool definitions array in `packages/app/vite.config.ts` (dev middleware, ~150 lines)
- `executeTool` switch statement in `packages/app/src/hooks/useChatHandler.ts`
- Per-tool handling in `api/chat.ts` (production route)
- Hard-coded system prompt string

### Replaced By

- `registry.toAnthropicTools()` for tool definitions
- `registry.execute(toolName, args, store)` for dispatch
- `registry.buildSystemPrompt(state, selection)` for system prompt
- Same pattern in both dev middleware and production route

### Chat Handler Changes

`useChatHandler.ts` simplifies to:
1. Build history from messages
2. Call `registry.buildSystemPrompt(...)` for system message
3. Call `streamChat(...)` with `registry.toAnthropicTools()`
4. On tool_use: call `registry.execute(name, input, store)`
5. Return tool_result to the AI

The MAX_TOOL_LOOPS, deferred execution, and parts tracking remain unchanged.

## 6. Future Integration Points

- **loonlang**: CRUD verbs map directly to language primitives. Type catalog becomes the language's type system.
- **MCP server**: `registry.toMcpTools()` replaces hand-maintained MCP tool list at mcp.vcad.io.
- **Command palette**: `registry.toPaletteItems()` generates palette entries with type-aware parameter forms.
- **New CsgOp variants**: Adding a variant to the Rust enum automatically creates a new tool — no TypeScript changes needed (unless custom executor logic is required).
