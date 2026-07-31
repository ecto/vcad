# Sketch Operations

Create 3D solids from 2D sketches using extrude, revolve, sweep, and loft operations.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Extrude/revolve in `crates/vcad-kernel-sketch`, sweep/loft in `crates/vcad-kernel-sweep`, evaluated in `packages/engine/src/evaluate.ts`.

## Problem

2D sketches alone cannot produce manufacturable parts. Users need to convert profile geometry into 3D solids to:

1. Create parts with consistent cross-sections (extrusions)
2. Generate rotationally symmetric features (revolves)
3. Build shapes that follow complex paths (sweeps)
4. Transition smoothly between different profiles (lofts)

Without these operations, users would be limited to primitives and booleans, unable to create organic or functionally shaped geometry like turbine blades, bottle necks, or structural beams.

## Solution

Four sketch-based operations that convert 2D profiles into 3D B-rep solids:

### Extrude

Sweep a closed profile along a linear direction to create a prismatic solid.

| Parameter | Type | Description |
|-----------|------|-------------|
| `plane` | `SketchPlane` | The plane containing the sketch |
| `origin` | `Vec3` | Origin point of the sketch |
| `segments` | `SketchSegment2D[]` | Closed profile geometry (lines, arcs) |
| `direction` | `Vec3` | Extrusion vector (includes distance) |

Creates a solid with:
- 2 cap faces (start and end profiles)
- N lateral faces (one per profile segment)

### Revolve

Rotate a closed profile around an axis to create a solid of revolution.

| Parameter | Type | Description |
|-----------|------|-------------|
| `plane` | `SketchPlane` | The plane containing the sketch |
| `origin` | `Vec3` | Origin point of the sketch |
| `segments` | `SketchSegment2D[]` | Closed profile geometry |
| `axisOrigin` | `Vec3` | Point on the rotation axis |
| `axisDir` | `Vec3` | Direction of the rotation axis |
| `angleDeg` | `number` | Rotation angle (0-360 degrees) |

Creates:
- Full 360-degree rotation: closed toroidal/cylindrical surfaces
- Partial rotation: wedge with cap faces at start/end angles

### Sweep

Move a closed profile along a 3D path curve with optional twist and scale.

| Parameter | Type | Description |
|-----------|------|-------------|
| `plane` | `SketchPlane` | The plane containing the sketch |
| `origin` | `Vec3` | Origin point of the sketch |
| `segments` | `SketchSegment2D[]` | Closed profile geometry |
| `path` | `PathCurve` | 3D path curve to sweep along |
| `twist_angle` | `number` | Total twist in radians (optional, default 0) |
| `scale_start` | `number` | Scale factor at path start (optional, default 1.0) |
| `scale_end` | `number` | Scale factor at path end (optional, default 1.0) |

Uses rotation-minimizing frames to maintain consistent profile orientation along the path. Supports helical paths for springs and threads.

### Loft

Interpolate between multiple profiles at different positions to create a smooth transition solid.

| Parameter | Type | Description |
|-----------|------|-------------|
| `profiles` | `Array<{plane, origin, segments}>` | Two or more profile definitions |
| `closed` | `boolean` | If true, connect last profile back to first (tube) |

Requirements:
- Minimum 2 profiles
- All profiles must have the same segment count
- Profiles should be positioned at different locations in 3D space

## UX Details

### Creation Flow

1. Create a sketch on a plane (face or reference plane)
2. Draw closed profile using sketch tools
3. Exit sketch mode
4. Select sketch operation from toolbar (Extrude, Revolve, Sweep, Loft)
5. Configure parameters in property panel
6. Live preview shows result in viewport
7. Confirm to add operation to feature tree

### Viewport Display

| State | Behavior |
|-------|----------|
| Previewing | Transparent mesh with operation parameters visible |
| Confirmed | Solid shaded geometry |
| Selected | Highlight color, parameters editable |

### Edge Cases

- **Empty profile**: Operation rejected, error shown
- **Open profile**: Warning displayed, operation may fail
- **Self-intersecting profile**: May produce invalid geometry
- **Zero-length extrusion**: Prevented by validation
- **Revolve axis through profile**: Creates hollow geometry
- **Mismatched loft profiles**: Error with specific mismatch info

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-sweep/src/lib.rs` | Module exports and error types |
| `crates/vcad-kernel-sweep/src/sweep.rs` | Sweep operation and SweepOptions |
| `crates/vcad-kernel-sweep/src/loft.rs` | Loft operation and LoftOptions |
| `crates/vcad-kernel-sweep/src/frenet.rs` | Rotation-minimizing frame calculation |
| `crates/vcad-kernel-wasm/src/lib.rs` | WASM bindings for all operations |
| `packages/core/src/stores/document-store.ts` | `addExtrude`, `addRevolve`, `addSweep`, `addLoft` |
| `packages/engine/src/evaluate.ts` | IR evaluation for sketch operations |
| `packages/ir/src/index.ts` | IR type definitions |

### Architecture

1. **Profile Processing**: Tessellate arcs into line segments for consistent vertex counts
2. **Frame Computation**: Calculate rotation-minimizing frames along path (sweep) or interpolate positions (loft)
3. **Vertex Grid**: Build 2D grid of vertices [path_position][profile_vertex]
4. **Face Construction**: Create quad faces connecting adjacent profile rings
5. **Cap Faces**: Close start/end with planar faces (unless closed loft)
6. **Twin Pairing**: Connect half-edge twins using quantized vertex positions
7. **Topology Assembly**: Build shell and solid from faces

### Key Algorithms

**Rotation-Minimizing Frames (sweep.rs)**
- Avoid Frenet frame issues (twisting at inflection points)
- Propagate frame orientation along path with minimal rotation
- Apply optional twist as angular interpolation

**Ruled Surface Lofting (loft.rs)**
- Connect corresponding vertices between profiles
- Create bilinear surface patches between profile edges
- Support both open and closed profile rings

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Extrude creates valid prismatic solid from closed profile
- [x] Revolve creates solid of revolution with configurable angle
- [x] Sweep follows 3D path with rotation-minimizing frames
- [x] Sweep supports twist and scale options
- [x] Loft interpolates between 2+ profiles
- [x] Loft supports closed option for tube shapes
- [x] All operations produce watertight B-rep topology
- [x] Half-edges are properly paired (no boundary edges)
- [x] Operations integrate with boolean pipeline
- [x] Operations can be filleted and chamfered
- [x] WASM bindings expose all operations to web app
- [x] Document store provides `addExtrude`, `addRevolve`, `addSweep`, `addLoft`

## Future Enhancements

- [ ] Draft angle on extrude (tapered walls for mold release)
- [ ] Symmetric extrude (both directions from sketch plane)
- [ ] Extrude to face (automatic distance calculation)
- [ ] Variable section sweep (scale/twist profile along path)
- [ ] Guide curves for loft (control lateral shape between profiles)
- [ ] Smooth B-spline surfaces for loft (currently uses ruled/planar)
- [ ] Thin wall extrude (shell operation built into extrude)
- [ ] Helix as first-class path type in UI
