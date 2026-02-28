# VCode Format

Terse text format for CAD operations that reduces token usage for AI models and file sizes.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p0` |
| Effort | `xs` (2-3 days) |
| Target | Phase 1 of cad0 |

## Problem

The current JSON-based IR format is verbose and wasteful:

1. **AI token waste** — LLMs spend tokens on JSON syntax (`"type":`, `{`, `}`, quotes) instead of semantic content
2. **Large file sizes** — A simple bracket with a hole is 50+ lines of JSON
3. **Poor human readability** — Nested structure obscures the actual operation sequence
4. **MCP overhead** — Every tool call includes verbose JSON payloads

Example: A box with a hole requires ~40 lines of JSON:
```json
{
  "version": "0.1",
  "nodes": {
    "1": {
      "id": 1,
      "name": "box",
      "op": { "type": "Cube", "size": { "x": 50, "y": 30, "z": 5 } }
    },
    "2": {
      "id": 2,
      "name": "hole",
      "op": { "type": "Cylinder", "radius": 5, "height": 10, "segments": 0 }
    },
    "3": {
      "id": 3,
      "name": null,
      "op": { "type": "Translate", "child": 2, "offset": { "x": 25, "y": 15, "z": 0 } }
    },
    "4": {
      "id": 4,
      "name": null,
      "op": { "type": "Difference", "left": 1, "right": 3 }
    }
  },
  "materials": { ... },
  "roots": [{ "root": 4, "material": "aluminum" }]
}
```

## Solution

A line-based VCode format where **line number = node ID**. Each line is a single operation with space-separated arguments.

The same box with hole in VCode format:
```
C 50 30 5          # Cube 50x30x5
Y 5 10             # Cylinder r=5, h=10
T 1 25 15 0        # Translate node 1 by (25, 15, 0)
D 0 2              # Difference: node 0 minus node 2
```

4 lines vs 40 lines. ~90% reduction in token usage.

### Format Specification

```
OPCODE [args...] [# comment]
```

- Lines are 0-indexed (first line = node 0)
- Arguments are space-separated
- Comments start with `#` and continue to end of line
- Empty lines and comment-only lines are skipped (don't create nodes)
- Node references use line numbers

### Operations Supported

**Primitives:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `C` | Cube | `width height depth` |
| `Y` | Cylinder | `radius height [segments]` |
| `S` | Sphere | `radius [segments]` |
| `K` | Cone | `bottom_radius top_radius height [segments]` |

**Booleans:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `U` | Union | `left_node right_node` |
| `D` | Difference | `left_node right_node` |
| `I` | Intersection | `left_node right_node` |

**Transforms:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `T` | Translate | `child_node x y z` |
| `R` | Rotate | `child_node rx ry rz` (degrees) |
| `X` | Scale | `child_node sx sy sz` |

**Patterns:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `LP` | LinearPattern | `child_node dx dy dz count spacing` |
| `CP` | CircularPattern | `child_node ox oy oz ax ay az count angle_deg` |

**Sketch Operations:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `SK` | Sketch start | `ox oy oz xx xy xz yx yy yz` (origin, x_dir, y_dir) |
| `L` | Line (in sketch) | `x1 y1 x2 y2` |
| `A` | Arc (in sketch) | `x1 y1 x2 y2 cx cy ccw` |
| `END` | Sketch end | (closes sketch block) |
| `E` | Extrude | `sketch_node dx dy dz` |
| `V` | Revolve | `sketch_node ox oy oz ax ay az angle_deg` |

**Modifiers:**
| Code | Operation | Arguments |
|------|-----------|-----------|
| `SH` | Shell | `child_node thickness` |

### Example: Complex Part

```
# Bracket with mounting holes
C 100 50 10                    # 0: base plate
Y 5 15                         # 1: hole template
T 1 20 25 0                    # 2: position hole 1
T 1 80 25 0                    # 3: position hole 2
D 0 2                          # 4: cut first hole
D 4 3                          # 5: cut second hole

# Add stiffener rib
SK 0 0 0 1 0 0 0 1 0           # 6: sketch on XY plane
L 0 0 100 0
L 100 0 100 20
L 100 20 0 20
L 0 20 0 0
END
E 6 0 0 5                      # 7: extrude 5mm
T 7 0 -10 10                   # 8: position rib
U 5 8                          # 9: union rib to bracket
```

**Not included in MVP:**
- Materials and scene entries (handled separately or via header)
- Assembly, joints, instances
- Named nodes (use comments)
- Imported geometry

## UX Details

### For AI Models (MCP)

New VCode format reduces prompt tokens by ~90%:

```
User: Create a bracket with two holes
AI: [uses create_cad_vcode tool]

C 50 30 5
Y 3 10
T 1 10 15 0
D 0 2
Y 3 10
T 4 40 15 0
D 3 5
```

### For File Storage

`.vcad` files can embed VCode in a header for compression:

```json
{
  "version": "0.1",
  "compact": "C 50 30 5\nY 5 10\nT 1 25 15 0\nD 0 2",
  "materials": { ... },
  "roots": [{ "root": 3, "material": "aluminum" }]
}
```

Or standalone `.vcadc` files for maximum compression.

### Error Handling

| Error | Behavior |
|-------|----------|
| Unknown opcode | Parse error with line number |
| Invalid node reference | Parse error: "node 5 referenced but only 3 nodes defined" |
| Wrong argument count | Parse error: "C expects 3 args, got 2" |
| Invalid number | Parse error with position |

## Implementation

### Files to Modify

| File | Changes |
|------|---------|
| `crates/vcad-ir/src/vcode.rs` | New module: `to_vcode()`, `from_vcode()` |
| `crates/vcad-ir/src/lib.rs` | Export compact module |
| `crates/vcad-kernel-wasm/src/lib.rs` | WASM bindings for compact functions |
| `packages/ir/src/index.ts` | TypeScript `toVCode()`, `fromVCode()` |
| `packages/ir/src/vcode.ts` | TypeScript implementation |
| `packages/mcp/src/tools/create.ts` | Add `create_cad_vcode` tool |

### Rust Implementation

```rust
// crates/vcad-ir/src/vcode.rs

/// Convert a Document to VCode format.
pub fn to_vcode(doc: &Document) -> String {
    let mut lines = Vec::new();

    // Sort nodes by ID to ensure correct line ordering
    let mut node_ids: Vec<_> = doc.nodes.keys().collect();
    node_ids.sort();

    for id in node_ids {
        let node = &doc.nodes[id];
        let line = op_to_vcode(&node.op);
        if let Some(name) = &node.name {
            lines.push(format!("{} # {}", line, name));
        } else {
            lines.push(line);
        }
    }

    lines.join("\n")
}

/// Parse VCode format into a Document.
pub fn from_vcode(input: &str) -> Result<Document, VCodeParseError> {
    let mut doc = Document::new();
    let mut node_id: NodeId = 0;

    for (line_num, line) in input.lines().enumerate() {
        let line = line.split('#').next().unwrap().trim();
        if line.is_empty() {
            continue;
        }

        let op = parse_op(line, line_num)?;
        doc.nodes.insert(node_id, Node {
            id: node_id,
            name: None,
            op,
        });
        node_id += 1;
    }

    // Default: last node is root with default material
    if !doc.nodes.is_empty() {
        doc.roots.push(SceneEntry {
            root: node_id - 1,
            material: "default".to_string(),
        });
    }

    Ok(doc)
}
```

### TypeScript Implementation

```typescript
// packages/ir/src/vcode.ts

export function toVCode(doc: Document): string {
  const lines: string[] = [];
  const sortedIds = Object.keys(doc.nodes)
    .map(Number)
    .sort((a, b) => a - b);

  for (const id of sortedIds) {
    const node = doc.nodes[id];
    lines.push(opToVCode(node.op));
  }

  return lines.join('\n');
}

export function fromVCode(input: string): Document {
  const doc: Document = { version: '0.1', nodes: {}, materials: {}, roots: [] };
  let nodeId = 0;

  for (const line of input.split('\n')) {
    const trimmed = line.split('#')[0].trim();
    if (!trimmed) continue;

    const op = parseOp(trimmed);
    doc.nodes[nodeId] = { id: nodeId, name: null, op };
    nodeId++;
  }

  if (nodeId > 0) {
    doc.roots.push({ root: nodeId - 1, material: 'default' });
  }

  return doc;
}
```

### WASM Bindings

```rust
// crates/vcad-kernel-wasm/src/lib.rs

#[wasm_bindgen]
pub fn ir_to_vcode(json: &str) -> Result<String, JsValue> {
    let doc: Document = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    Ok(vcad_ir::vcode::to_vcode(&doc))
}

#[wasm_bindgen]
pub fn ir_from_vcode(compact: &str) -> Result<String, JsValue> {
    let doc = vcad_ir::vcode::from_vcode(compact)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    serde_json::to_string(&doc)
        .map_err(|e| JsValue::from_str(&e.to_string()))
}
```

## Tasks

### Phase 1: Core Parser (Rust)

- [ ] Create `crates/vcad-ir/src/vcode.rs` module (`xs`)
- [ ] Implement `to_vcode()` for all `CsgOp` variants (`s`)
- [ ] Implement `from_vcode()` parser (`s`)
- [ ] Add sketch block parsing (SK...END) (`xs`)
- [ ] Add comprehensive unit tests (`xs`)
- [ ] Export from `lib.rs` (`xs`)

### Phase 2: WASM & TypeScript

- [ ] Add WASM bindings `ir_to_vcode`, `ir_from_vcode` (`xs`)
- [ ] Create `packages/ir/src/vcode.ts` (`s`)
- [ ] Add TypeScript tests (`xs`)
- [ ] Update `@vcad/ir` exports (`xs`)

### Phase 3: Integration

- [ ] Add `create_cad_vcode` MCP tool (`xs`)
- [ ] Add VCode format to CLI (`vcad vcode input.vcad`) (`xs`)
- [ ] Documentation for format spec (`xs`)

### Phase 4: Validation

- [ ] Round-trip tests for all IR types (`s`)
- [ ] Fuzz testing with `cargo-fuzz` (`s`)
- [ ] Benchmark token reduction vs JSON (`xs`)

## Acceptance Criteria

- [ ] `to_vcode()` produces valid VCode format for all `CsgOp` variants
- [ ] `from_vcode()` parses VCode format back to equivalent `Document`
- [ ] Round-trip `doc -> compact -> doc` preserves all geometry operations
- [ ] Sketch blocks (SK...END) parse correctly with nested segments
- [ ] Error messages include line numbers and descriptive text
- [ ] WASM bindings work in browser
- [ ] TypeScript implementation matches Rust behavior
- [ ] Fuzz testing passes 1M+ iterations without panics
- [ ] Token usage reduced by >80% compared to JSON for typical models

## Future Enhancements

- [ ] Binary VCode format for maximum compression
- [ ] Materials header section (`@MAT aluminum 0.91 0.92 0.93 1.0 0.4`)
- [ ] Named node references (`$hole` instead of line numbers)
- [ ] Assembly support (instances, joints)
- [ ] Streaming parser for large files
- [ ] VSCode syntax highlighting extension
- [ ] Compact format as primary MCP input (deprecate JSON)
