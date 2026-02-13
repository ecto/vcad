---
name: vcad-step-import
description: Import and work with STEP files using vcad MCP tools. Use when the user mentions STEP files, .step, .stp, importing CAD from other software (Fusion 360, SolidWorks, Onshape), or converting between CAD formats.
---

# vcad STEP Import

Import geometry from STEP files (AP203/AP214) using the vcad MCP tools. STEP files are the standard interchange format exported by Fusion 360, SolidWorks, Onshape, CATIA, and other CAD software.

## Available Tools

| Tool | Purpose |
|------|---------|
| `import_step` | Import geometry from a .step/.stp file |
| `inspect_cad` | Analyze imported geometry (volume, area, bbox) |
| `export_cad` | Re-export to STL or GLB |
| `open_in_browser` | View imported model in vcad.io |

## import_step

```json
{
  "filename": "part.step",
  "name": "imported_part",
  "material": "steel"
}
```

Parameters:
- `filename` — path to the STEP file (.step or .stp)
- `name` — part name (default: filename without extension)
- `material` — material key: `"steel"` (default, density 7850 kg/m3), `"aluminum"` (2700), or `"default"`

Returns an IR document with `ImportedMesh` nodes containing triangulated geometry, plus a summary:

```json
{
  "document": { ... },
  "summary": {
    "bodies": 3,
    "total_triangles": 15420,
    "total_vertices": 8210
  }
}
```

## Workflow

### Import → Inspect → Export

```
1. import_step(filename: "bracket.step")
   → Get IR document + body count

2. inspect_cad(ir: <document>)
   → Volume, surface area, bounding box, mass

3. export_cad(ir: <document>, filename: "bracket.stl")
   → STL file for 3D printing
```

### Import → View in Browser

```
1. import_step(filename: "housing.step")
   → Get IR document

2. open_in_browser(document: <compact IR>)
   → Shareable vcad.io URL
```

Note: `open_in_browser` works best with smaller models. Very large imports may exceed URL length limits — use `export_cad` instead.

## Supported Formats

| Format | Support |
|--------|---------|
| AP203 | Full |
| AP214 | Full |
| AP242 | Partial (geometry only) |

Common sources: Fusion 360, SolidWorks, Onshape, CATIA, Creo, Inventor, FreeCAD, Rhino.

## What Gets Imported

STEP import produces triangulated meshes (ImportedMesh nodes), not parametric BRep. This means:
- Geometry is viewable and exportable
- Volume and area measurements work
- Boolean operations with imported meshes are not supported
- The parametric history from the source CAD is not preserved

For parametric editing, recreate the geometry using `create_cad_document`.

## Tips

- Use absolute paths if the file is not in the current working directory
- Multi-body STEP files produce multiple ImportedMesh nodes (one per body)
- Assign `"material": "steel"` or `"aluminum"` to get mass calculations from `inspect_cad`
- Large STEP files (>50MB) may take several seconds to process
