# Primitives

Create basic 3D shapes (box, cylinder, sphere, cone) as parametric building blocks for 3D modeling.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Correction: all four primitives live in `crates/vcad-kernel-primitives/src/lib.rs` — there are no separate `box.rs`/`cylinder.rs`/`sphere.rs`/`cone.rs` files.

## Problem

Every 3D model starts from basic shapes. Without parametric primitives, users would need to:

1. Import pre-made geometry from external files
2. Manually construct shapes from vertices and faces
3. Use complex sweep operations for simple forms

CAD workflows depend on having reliable, dimension-controllable building blocks that can be combined via booleans and modified through operations like fillets and chamfers.

## Solution

Four parametric primitives with customizable dimensions, centered at origin by default:

### Box

Rectangular cuboid defined by three orthogonal dimensions.

| Parameter | Type | Description |
|-----------|------|-------------|
| `width` | `f64` | Size along X axis (mm) |
| `height` | `f64` | Size along Y axis (mm) |
| `depth` | `f64` | Size along Z axis (mm) |

### Cylinder

Circular extrusion with configurable tessellation.

| Parameter | Type | Description |
|-----------|------|-------------|
| `radius` | `f64` | Radius of circular cross-section (mm) |
| `height` | `f64` | Length along axis (mm) |
| `segments` | `u32` | Number of facets around circumference |

### Sphere

Spherical surface with UV tessellation.

| Parameter | Type | Description |
|-----------|------|-------------|
| `radius` | `f64` | Radius from center (mm) |
| `segments` | `u32` | Number of subdivisions (affects both latitude and longitude) |

### Cone

Truncated cone (frustum) supporting both pointed cones and flat-topped variants.

| Parameter | Type | Description |
|-----------|------|-------------|
| `bottom_radius` | `f64` | Radius at base (mm) |
| `top_radius` | `f64` | Radius at top (mm), 0 for pointed cone |
| `height` | `f64` | Height along axis (mm) |

## UX Details

### Creation Flow

1. Select primitive type from toolbar or menu
2. Shape appears at origin with default dimensions
3. Property panel shows editable parameters
4. Scrub or type values to adjust dimensions
5. Live preview updates in viewport

### Viewport Display

| State | Behavior |
|-------|----------|
| Default | Solid shaded with edges |
| Selected | Highlight color, transform gizmo visible |
| Editing | Live mesh updates on parameter change |

### Edge Cases

- **Zero dimensions**: Prevented by validation (minimum 0.001mm)
- **Very large values**: Warn if dimension exceeds 10m
- **Low segment count**: Minimum 3 for cylinder/cone, 4 for sphere

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-primitives/src/lib.rs` | Core primitive generation algorithms |
| `crates/vcad-kernel-primitives/src/box.rs` | Box BRep construction |
| `crates/vcad-kernel-primitives/src/cylinder.rs` | Cylinder BRep construction |
| `crates/vcad-kernel-primitives/src/sphere.rs` | Sphere BRep construction |
| `crates/vcad-kernel-primitives/src/cone.rs` | Cone BRep construction |
| `crates/vcad-kernel-wasm/src/lib.rs` | WASM bindings exposing primitives to JS |
| `packages/core/src/types.ts` | TypeScript types for primitive parameters |
| `packages/ir/src/index.ts` | IR operation definitions |
| `packages/engine/src/evaluate.ts` | Evaluation of primitive operations |

### Architecture

Primitives are implemented in the Rust kernel using half-edge BRep topology:

1. **Geometry**: Analytic surfaces (Plane, Cylinder, Cone, Sphere)
2. **Topology**: Vertices, edges, faces with proper connectivity
3. **Tessellation**: Convert BRep to triangle mesh for display
4. **WASM**: Serialize mesh data for transfer to JavaScript

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Box primitive creates valid BRep with 6 faces
- [x] Cylinder primitive creates capped cylinder with configurable segments
- [x] Sphere primitive creates closed surface with UV parameterization
- [x] Cone primitive supports both pointed and truncated forms
- [x] All primitives pass topology validation (closed, manifold)
- [x] Parameters are editable in property panel
- [x] Live preview updates viewport during parameter editing
- [x] Primitives work as inputs to boolean operations
- [x] Primitives can be filleted and chamfered
- [x] WASM bindings expose all primitives to web app

## Future Enhancements

- [ ] Torus primitive (ring shape)
- [ ] Wedge/prism primitive
- [ ] Polygon-based prism (arbitrary cross-section)
- [ ] Ellipsoid primitive
- [ ] Rounded box (box with fillet edges built-in)
- [ ] Helix/spring primitive
