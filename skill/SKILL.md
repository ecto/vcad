---
name: vcad
description: Create 3D CAD models using vcad MCP tools. Use when the user asks to create 3D parts, mechanical components, plates with holes, brackets, or any parametric geometry. Supports primitives (cube, cylinder, sphere, cone), boolean operations, transforms, patterns, and export to STL/GLB.
---

# vcad - Parametric CAD for AI Agents

Create 3D CAD models programmatically using the vcad MCP tools.

## Available Tools

### create_cad_document
Build geometry from structured primitives and operations.

### export_cad
Export to STL (3D printing) or GLB (visualization).

### inspect_cad
Get volume, surface area, bounding box, and triangle count.

## Primitive Origins

Understanding where primitives are positioned is critical for correct placement:

| Primitive | Origin | Extent |
|-----------|--------|--------|
| Cube | Corner at (0,0,0) | Extends to (size.x, size.y, size.z) |
| Cylinder | Base center at (0,0,0) | Height along +Z |
| Sphere | Center at (0,0,0) | Radius in all directions |
| Cone | Base center at (0,0,0) | Height along +Z |

## Positioning Operations

The `at` parameter accepts three formats:

1. **Absolute**: `{x: 25, y: 15, z: 0}` - exact coordinates in mm
2. **Named**: `"center"`, `"top-center"`, `"bottom-center"` - relative to base primitive
3. **Percentage**: `{x: "50%", y: "50%"}` - percentage of base primitive bounds

## Common Patterns

### Plate with Centered Hole

```json
{
  "parts": [{
    "name": "plate",
    "primitive": {"type": "cube", "size": {"x": 50, "y": 50, "z": 5}},
    "operations": [
      {"type": "hole", "diameter": 6, "at": "center"}
    ]
  }]
}
```

### Plate with Corner Holes

```json
{
  "parts": [{
    "name": "mounting_plate",
    "primitive": {"type": "cube", "size": {"x": 100, "y": 60, "z": 5}},
    "operations": [
      {"type": "hole", "diameter": 4, "at": {"x": 10, "y": 10}},
      {"type": "hole", "diameter": 4, "at": {"x": 90, "y": 10}},
      {"type": "hole", "diameter": 4, "at": {"x": 10, "y": 50}},
      {"type": "hole", "diameter": 4, "at": {"x": 90, "y": 50}}
    ]
  }]
}
```

### Cylinder with Blind Hole

```json
{
  "parts": [{
    "name": "bushing",
    "primitive": {"type": "cylinder", "radius": 15, "height": 20},
    "operations": [
      {"type": "hole", "diameter": 10, "depth": 15, "at": "center"}
    ]
  }]
}
```

### L-Bracket

```json
{
  "parts": [{
    "name": "bracket",
    "primitive": {"type": "cube", "size": {"x": 40, "y": 40, "z": 5}},
    "operations": [
      {"type": "union", "primitive": {"type": "cube", "size": {"x": 5, "y": 40, "z": 30}}, "at": {"x": 0, "y": 0, "z": 5}},
      {"type": "hole", "diameter": 5, "at": {"x": 20, "y": 20}},
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 2.5, "height": 40}, "at": {"x": 2.5, "y": 20, "z": 20}}
    ]
  }]
}
```

### Linear Pattern of Holes

```json
{
  "parts": [{
    "name": "rail",
    "primitive": {"type": "cube", "size": {"x": 200, "y": 20, "z": 10}},
    "operations": [
      {
        "type": "difference",
        "primitive": {"type": "cylinder", "radius": 3, "height": 15},
        "at": {"x": 20, "y": 10, "z": -2}
      },
      {
        "type": "linear_pattern",
        "direction": {"x": 1, "y": 0, "z": 0},
        "count": 5,
        "spacing": 40
      }
    ]
  }]
}
```

### Circular Pattern

```json
{
  "parts": [{
    "name": "flange",
    "primitive": {"type": "cylinder", "radius": 40, "height": 10},
    "operations": [
      {"type": "hole", "diameter": 20, "at": "center"},
      {
        "type": "difference",
        "primitive": {"type": "cylinder", "radius": 4, "height": 15},
        "at": {"x": 30, "y": 0, "z": -2}
      },
      {
        "type": "circular_pattern",
        "axis_origin": {"x": 0, "y": 0, "z": 0},
        "axis_dir": {"x": 0, "y": 0, "z": 1},
        "count": 6,
        "angle_deg": 360
      }
    ]
  }]
}
```

## Workflow

1. **Create**: Use `create_cad_document` to build geometry
2. **Inspect**: Use `inspect_cad` to verify dimensions and volume
3. **Export**: Use `export_cad` to save as `.stl` or `.glb`

## Tips

- Holes default to through-holes; specify `depth` for blind holes
- Use percentage positioning for parametric designs that scale
- Cube origin is at corner - add half-size to center operations
- Cylinder/cone origins are at base center - no X/Y offset needed for centered holes
- Always extend cutting cylinders past the part (the hole operation does this automatically)

## Setup

**Recommended:** Install the vcad Claude Code plugin for zero-config setup (includes MCP server, skills, and slash commands):

```bash
claude plugin add vcad
```

**Manual setup:** Add the vcad MCP server to your Claude Code config:

```json
{
  "mcpServers": {
    "vcad": {
      "command": "npx",
      "args": ["-y", "@vcad/mcp"]
    }
  }
}
```

Or run from local checkout:

```json
{
  "mcpServers": {
    "vcad": {
      "command": "node",
      "args": ["packages/mcp/dist/index.js"]
    }
  }
}
```
