---
name: vcad
description: Create 3D CAD models using vcad MCP tools. Use when the user asks to create 3D parts, mechanical components, enclosures, brackets, gears, or any parametric geometry. Supports primitives, sketch-based operations (extrude, revolve, sweep, loft), booleans, patterns, fillets, shell, assemblies, and export to STL/GLB.
---

# vcad - Parametric CAD for AI Agents

Create 3D CAD models programmatically using the vcad MCP tools. All dimensions are in millimeters.

## Available Tools

| Tool | Purpose |
|------|---------|
| `create_cad_document` | Build geometry from primitives, sketches, and operations |
| `export_cad` | Export to STL (3D printing) or GLB (visualization) |
| `inspect_cad` | Get volume, surface area, bounding box, center of mass, mass |
| `import_step` | Import geometry from STEP files |
| `open_in_browser` | Generate shareable vcad.io URL |
| `get_changelog` | Query recent changes and features |

For assembly and physics tools, see the `assembly-physics` skill.

## Output Format

Use `"format": "vcode"` (default) for token-efficient output (~5x smaller). Use `"format": "json"` for human-readable debugging.

## Part Types

Each part must use exactly ONE base geometry type:

### Primitive-based

```json
{
  "name": "plate",
  "primitive": {"type": "cube", "size": {"x": 100, "y": 60, "z": 5}},
  "operations": [...]
}
```

### Extrude-based (sketch → solid)

```json
{
  "name": "bracket",
  "extrude": {
    "sketch": {
      "plane": "xy",
      "shape": {"type": "rectangle", "width": 40, "height": 20, "centered": true}
    },
    "height": 10
  }
}
```

### Revolve-based (sketch → solid of revolution)

```json
{
  "name": "vase",
  "revolve": {
    "sketch": {
      "plane": "xy",
      "shape": {
        "type": "polygon",
        "points": [
          {"x": 10, "y": 0}, {"x": 15, "y": 20},
          {"x": 12, "y": 40}, {"x": 14, "y": 50},
          {"x": 0, "y": 50}, {"x": 0, "y": 0}
        ]
      }
    },
    "axis": "y",
    "angle_deg": 360
  }
}
```

### Sweep-based (sketch along path)

```json
{
  "name": "tube",
  "sweep": {
    "sketch": {
      "shape": {"type": "circle", "radius": 5}
    },
    "path": {"type": "line", "start": {"x": 0, "y": 0, "z": 0}, "end": {"x": 50, "y": 0, "z": 30}}
  }
}
```

Helix path for springs/threads:

```json
"path": {"type": "helix", "radius": 20, "pitch": 10, "height": 60}
```

Optional sweep parameters: `twist_deg`, `scale_start`, `scale_end`.

### Loft-based (interpolate between sketches)

```json
{
  "name": "transition",
  "loft": {
    "sketches": [
      {"plane": "xy", "at": {"x": 0, "y": 0, "z": 0}, "shape": {"type": "rectangle", "width": 40, "height": 40, "centered": true}},
      {"plane": "xy", "at": {"x": 0, "y": 0, "z": 30}, "shape": {"type": "circle", "radius": 15}}
    ]
  }
}
```

## Sketch Shapes

Sketches are used by extrude, revolve, sweep, and loft operations:

| Shape | Parameters |
|-------|-----------|
| `rectangle` | `width`, `height`, `centered` (bool, default false) |
| `circle` | `radius` |
| `polygon` | `points` (array of `{x, y}`), `closed` (default true) |

Sketch planes: `"xy"` (default), `"xz"`, `"yz"`. Origin set with `"at": {x, y, z}`.

## Primitive Origins

| Primitive | Origin | Extent |
|-----------|--------|--------|
| Cube | Corner at (0,0,0) | Extends to (size.x, size.y, size.z) |
| Cylinder | Base center at (0,0,0) | Height along +Z |
| Sphere | Center at (0,0,0) | Radius in all directions |
| Cone | Base center at (0,0,0) | Height along +Z, `radius_bottom`/`radius_top` |

## Positioning

The `at` parameter for boolean operations and holes:

1. **Absolute**: `{"x": 25, "y": 15, "z": 0}` — exact coordinates in mm
2. **Named**: `"center"`, `"top-center"`, `"bottom-center"` — relative to base primitive
3. **Percentage**: `{"x": "50%", "y": "50%"}` — percentage of base primitive bounds

## Operations

Applied in order after the base geometry:

| Operation | Key Parameters |
|-----------|---------------|
| `union` | `primitive`, `at` |
| `difference` | `primitive`, `at` |
| `intersection` | `primitive`, `at` |
| `hole` | `diameter`, `at`, `depth` (omit for through-hole) |
| `translate` | `offset: {x, y, z}` |
| `rotate` | `angles: {x, y, z}` (degrees) |
| `scale` | `factor: {x, y, z}` |
| `linear_pattern` | `direction: {x, y, z}`, `count`, `spacing` |
| `circular_pattern` | `axis_origin`, `axis_dir`, `count`, `angle_deg` |
| `fillet` | `radius` — rounds all edges |
| `chamfer` | `distance` — bevels all edges |
| `shell` | `thickness` — hollows the part |

## Materials

Assign via `"material"` on each part: `"steel"` (density 7850), `"aluminum"` (2700), `"default"`.

`inspect_cad` reports mass when density is available.

## Common Patterns

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

### Flange with Circular Bolt Pattern

```json
{
  "parts": [{
    "name": "flange",
    "primitive": {"type": "cylinder", "radius": 40, "height": 10},
    "operations": [
      {"type": "hole", "diameter": 20, "at": "center"},
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 4, "height": 15}, "at": {"x": 30, "y": 0, "z": -2}},
      {"type": "circular_pattern", "axis_origin": {"x": 0, "y": 0, "z": 0}, "axis_dir": {"x": 0, "y": 0, "z": 1}, "count": 6, "angle_deg": 360}
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
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 3, "height": 15}, "at": {"x": 20, "y": 10, "z": -2}},
      {"type": "linear_pattern", "direction": {"x": 1, "y": 0, "z": 0}, "count": 5, "spacing": 40}
    ]
  }]
}
```

### Extruded T-Profile

```json
{
  "parts": [{
    "name": "t_beam",
    "extrude": {
      "sketch": {
        "plane": "xz",
        "shape": {
          "type": "polygon",
          "points": [
            {"x": -25, "y": 0}, {"x": 25, "y": 0},
            {"x": 25, "y": 3}, {"x": 3, "y": 3},
            {"x": 3, "y": 30}, {"x": -3, "y": 30},
            {"x": -3, "y": 3}, {"x": -25, "y": 3}
          ]
        }
      },
      "height": 100
    }
  }]
}
```

### Shelled Box (Enclosure)

```json
{
  "parts": [{
    "name": "enclosure",
    "primitive": {"type": "cube", "size": {"x": 80, "y": 60, "z": 40}},
    "operations": [
      {"type": "shell", "thickness": 2}
    ]
  }]
}
```

### Filleted Part

```json
{
  "parts": [{
    "name": "rounded_block",
    "primitive": {"type": "cube", "size": {"x": 30, "y": 20, "z": 15}},
    "operations": [
      {"type": "fillet", "radius": 2}
    ]
  }]
}
```

### Spring (Helix Sweep)

```json
{
  "parts": [{
    "name": "spring",
    "sweep": {
      "sketch": {"shape": {"type": "circle", "radius": 1.5}},
      "path": {"type": "helix", "radius": 10, "pitch": 8, "height": 40}
    },
    "material": "steel"
  }]
}
```

## Workflow

1. **Create** — `create_cad_document` to build geometry
2. **Inspect** — `inspect_cad` to verify dimensions, volume, and mass
3. **Export** — `export_cad` to save as `.stl` or `.glb`
4. **Share** — `open_in_browser` to generate a vcad.io URL

## Tips

- Holes default to through-holes; specify `depth` for blind holes
- Use percentage positioning for parametric designs that scale
- Cube origin is at corner — add half-size to center operations
- Cylinder/cone origins are at base center — no X/Y offset needed for centered holes
- Always extend cutting cylinders past the part (the `hole` operation does this automatically)
- `fillet` and `chamfer` apply to all edges — use them as the last operation
- `shell` hollows the entire part — combine with `difference` to remove specific faces
- For multi-part designs, define each part in the `parts` array with a unique `name`
- Use `"format": "vcode"` (default) to save tokens in the response
