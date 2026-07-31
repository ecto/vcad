# Import/Export

Exchange geometry with other CAD tools, 3D printers, and renderers.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (core complete) |

> Verified against repo 2026-07-30. Corrections: STEP export is no longer primitives-only — full multi-body BRep export with analytic edge reconstruction shipped (`crates/vcad-kernel-step`, `packages/core/src/utils/export-step.ts`); web STEP export is wired (no longer "coming soon"); STEP import via drag-drop/file picker shipped in the app (`App.tsx` accepts `.step/.stp`); DXF export shipped (drafting + sheet metal). The "shipped (partial UI)" caveats below are stale.

## Problem

CAD models don't exist in isolation. Users need to:

1. **3D print** their designs (requires STL or similar mesh format)
2. **Exchange with other CAD tools** (STEP is the industry standard for B-rep)
3. **Render in game engines or visualization tools** (GLB/glTF for Three.js, Blender, Unity)
4. **Import existing designs** from other CAD systems
5. **Collaborate** with engineers using different software

Without robust import/export, vcad is a walled garden. Users won't adopt a CAD tool that traps their work.

## Solution

Multiple format support covering the primary use cases:

### Export Formats

| Format | Purpose | Status |
|--------|---------|--------|
| **STL** | 3D printing, mesh exchange | Shipped |
| **GLB** | Game engines, renderers, web viewers | Shipped |
| **STEP** | CAD exchange (AP214 B-rep) | Shipped (full B-rep, multi-body) |

### Import Formats

| Format | Purpose | Status |
|--------|---------|--------|
| **STEP** | Import B-rep from other CAD tools | Shipped (kernel + CLI) |
| **STL** | Import mesh files | Shipped |

### CLI Commands

```bash
# Export to various formats
vcad export input.vcad output.stl
vcad export input.vcad output.glb
vcad export input.vcad output.step

# Import STEP file
vcad import input.step output.vcad --name "Imported Part"

# Get file info
vcad info input.vcad
```

### Web App

Export dropdown in toolbar with options:
- Build (manufacturing quote)
- Export STL
- Export GLB
- Export STEP (disabled, coming soon)

## UX Details

### Export Flow

1. User clicks export button or selects from dropdown
2. File downloads immediately with default name `model.{ext}`
3. Toast notification confirms export

| State | Behavior |
|-------|----------|
| No parts | Export button disabled |
| Click STL | Downloads `model.stl` immediately |
| Click GLB | Downloads `model.glb` immediately |
| Click STEP | Shows "coming soon" toast (not yet wired to kernel) |

### Import Flow (CLI)

```bash
vcad import part.step mypart.vcad
```

1. Parse STEP file using kernel
2. Convert to vcad document format
3. Write output file
4. Print success message with part count

### Edge Cases

- **Empty document**: Prevent export, show disabled state
- **Large models**: No progress indicator yet (future enhancement)
- **STEP export limitations**: Only primitives supported; boolean results convert to mesh which cannot round-trip to STEP
- **Invalid STEP files**: Parser returns descriptive error messages

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-step/src/lib.rs` | STEP parser/writer entry point |
| `crates/vcad-kernel-step/src/reader.rs` | `read_step()`, `read_step_from_buffer()` |
| `crates/vcad-kernel-step/src/writer.rs` | `write_step()`, `write_step_to_buffer()` |
| `crates/vcad-kernel-step/src/parser.rs` | STEP file tokenization and entity parsing |
| `crates/vcad-kernel-step/src/lexer.rs` | STEP file lexical analysis |
| `crates/vcad-kernel-step/src/entities/` | STEP entity types (curves, surfaces, topology) |
| `crates/vcad-cli/src/main.rs` | CLI `export` and `import` commands |
| `packages/core/src/utils/export-stl.ts` | `exportStlBuffer()`, `exportStlBlob()` |
| `packages/core/src/utils/export-gltf.ts` | `exportGltfBuffer()`, `exportGltfBlob()` |
| `packages/core/src/utils/import-stl.ts` | `parseStl()` for binary and ASCII STL |
| `packages/app/src/components/BottomToolbar.tsx` | Build/Export toolbar UI |
| `packages/app/src/stores/output-store.ts` | Quote panel state and pricing helpers |

### Architecture

**STEP (Rust Kernel)**
```
STEP File → Lexer → Parser → STEP Entities → BRep Solid
BRep Solid → Writer → STEP Entities → STEP File
```

AP214 (Automotive Design) protocol targeting mechanical CAD interoperability.

**STL Export (TypeScript)**
```
EvaluatedScene → exportStlBuffer() → Binary STL ArrayBuffer → Blob → Download
```

Binary format: 80-byte header + uint32 triangle count + 50 bytes per triangle (normal + 3 vertices + attribute).

**GLB Export (TypeScript)**
```
EvaluatedScene → exportGltfBuffer() → GLB ArrayBuffer → Blob → Download
```

Minimal glTF 2.0 binary: merged positions + indices, single mesh, no materials.

**STL Import (TypeScript)**
```
ArrayBuffer → parseStl() → TriangleMesh { positions, indices }
```

Supports both ASCII and binary STL formats. Deduplicates vertices using position hash map.

## Tasks

### Completed

- [x] STEP lexer and parser (`xs`)
- [x] STEP reader for B-rep import (`m`)
- [x] STEP writer for B-rep export (`m`)
- [x] STEP entity types (curves, surfaces, topology) (`m`)
- [x] CLI `export` command for STL/STEP (`s`)
- [x] CLI `import` command for STEP (`s`)
- [x] TypeScript STL export (`s`)
- [x] TypeScript GLB export (`s`)
- [x] TypeScript STL import (`s`)
- [x] Web app export dropdown UI (`s`)
- [x] Export toast notifications (`xs`)

### Remaining

- [x] Web UI for STEP import drag-and-drop (`s`)
- [x] Wire STEP export to web app (`s`)
- [x] DXF export for 2D drawings (`m`)
- [x] PDF export for drawings (`m`)
- [ ] Export progress indicator for large models (`xs`)
- [ ] Custom filename on export (`xs`)

## Acceptance Criteria

- [x] `vcad export input.vcad output.stl` produces valid binary STL
- [x] `vcad export input.vcad output.step` produces valid AP214 STEP for primitive shapes
- [x] `vcad import input.step output.vcad` imports STEP B-rep to vcad document
- [x] Web app "Export STL" downloads valid STL file
- [x] Web app "Export GLB" downloads valid GLB loadable in Three.js/Blender
- [x] STL import parses both ASCII and binary formats
- [x] STEP parser handles common AP214 entities (planes, cylinders, spheres, cones)
- [x] Web app can import STEP files via file picker or drag-and-drop
- [x] 2D drawings can export to DXF for laser cutting

## Future Enhancements

- [ ] OBJ export (widely supported mesh format)
- [ ] 3MF export (modern 3D printing format with metadata)
- [ ] IGES import/export (legacy CAD format)
- [ ] Mesh simplification before STL export
- [ ] Per-part color/material in GLB export
- [ ] Batch export (multiple files at once)
- [ ] Cloud-based format conversion service
- [ ] STEP export for boolean results (requires B-rep boolean output)
