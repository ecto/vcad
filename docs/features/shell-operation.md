# Shell Operation

Hollow out solid geometry by offsetting faces inward, creating thin-walled parts.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Shell op present in `crates/vcad-kernel-shell`, `CsgOp::Shell` in vcad-ir, evaluated in `packages/engine/src/evaluate.ts`.

## Problem

Creating hollow parts (enclosures, containers, housings) is a fundamental CAD operation. Without a shell command, users must:

1. Create an inner solid manually with offset dimensions
2. Position it precisely inside the outer solid
3. Perform a boolean subtraction
4. Manually adjust each face for uniform wall thickness

This is error-prone and tedious, especially for complex geometry where computing inward offsets by hand is difficult. Every mechanical enclosure, electronics housing, or container requires this workflow.

## Solution

A shell operation that hollows a solid by offsetting all faces inward by a specified thickness:

| Parameter | Type | Description |
|-----------|------|-------------|
| `thickness` | `f64` | Wall thickness (positive = inward offset, mm) |

The operation creates:
- Outer shell: original solid surface
- Inner shell: offset surface (reversed normals)
- Connection: outer and inner shells form a closed manifold

### Algorithm

1. Tessellate the input B-rep to a triangle mesh
2. Compute vertex normals (average of adjacent face normals)
3. Offset each vertex inward by `thickness * vertex_normal`
4. Create inner shell with reversed winding order
5. Combine outer and inner shells into a single manifold
6. Convert back to B-rep representation

## UX Details

### Creation Flow

1. Select a solid in the viewport or feature tree
2. Choose Shell from the operations menu or toolbar
3. Enter wall thickness in the property panel
4. Live preview shows the resulting hollow part
5. Commit the operation

### Viewport Display

| State | Behavior |
|-------|----------|
| Preview | Semi-transparent inner surface visible |
| Applied | Solid rendering with interior visible at open faces |
| Section view | Wall thickness clearly visible |

### Edge Cases

- **Zero thickness**: Prevented by validation (minimum 0.001mm)
- **Thickness > half smallest dimension**: Warning shown, may create invalid geometry
- **Non-manifold input**: Operation fails with error message
- **Complex curved surfaces**: Uses mesh approximation for offset

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-shell/src/lib.rs` | Core shell algorithm |
| `crates/vcad-ir/src/lib.rs` | `CsgOp::Shell` IR definition |
| `packages/ir/src/index.ts` | `ShellOp` TypeScript type |
| `packages/engine/src/evaluate.ts` | Shell operation evaluation |
| `crates/vcad-kernel-wasm/src/lib.rs` | WASM bindings for shell |

### IR Definition

```rust
/// Shell - hollow out a solid by offsetting faces.
Shell {
    /// Child node to shell.
    child: NodeId,
    /// Wall thickness (inward offset).
    thickness: f64,
}
```

### Algorithm Details

The implementation uses a mesh-based approach:

1. **Vertex normals**: Computed as weighted average of adjacent face normals
2. **Inward offset**: Each vertex moves along its negative normal by thickness
3. **Winding reversal**: Inner shell triangles have reversed winding for correct normals
4. **B-rep conversion**: Result mesh converted back to B-rep with twin half-edge pairing

This is a Phase 1 simplification. Full B-rep shelling would offset surfaces analytically and handle self-intersections.

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Shell operation creates hollow solid from input geometry
- [x] Wall thickness parameter controls offset distance
- [x] Resulting geometry is manifold (closed, valid topology)
- [x] Shell volume is less than original solid volume
- [x] Works with box, cylinder, sphere, and cone primitives
- [x] Works on boolean operation results
- [x] Property panel allows thickness editing
- [x] Live preview updates during parameter changes
- [x] WASM bindings expose shell to web app

## Future Enhancements

- [ ] Variable thickness (different thickness per face)
- [ ] Open faces (remove specified faces to create openings)
- [ ] Rib generation (internal reinforcement structures)
- [ ] Analytic surface offsetting for curved faces
- [ ] Self-intersection detection and handling
- [ ] Draft angle support for moldable parts
