# Transforms

Move, rotate, and scale geometry in 3D space.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Correction: Mirror is now shipped (`CsgOp::Mirror` in `crates/vcad-ir/src/lib.rs`, `addMirror` in `packages/core/src/stores/document-store.ts`) — the "Mirror not included" note below is stale.

## Problem

Parts need to be positioned and oriented in 3D space. Without transforms, every primitive would be stuck at the origin with default orientation. Users need:

1. **Translation** — Move parts to specific locations
2. **Rotation** — Orient parts at angles
3. **Scale** — Resize parts non-uniformly

These are fundamental operations required before any meaningful modeling can happen.

## Solution

Three transform operations that chain together in a consistent order:

### Translation

Offset geometry along X, Y, Z axes.

```typescript
{
  type: "Translate",
  child: NodeId,
  offset: { x: number, y: number, z: number }  // mm
}
```

### Rotation

Rotate geometry using Euler angles (applied as X, then Y, then Z).

```typescript
{
  type: "Rotate",
  child: NodeId,
  angles: { x: number, y: number, z: number }  // degrees
}
```

### Scale

Non-uniform scaling per axis.

```typescript
{
  type: "Scale",
  child: NodeId,
  factor: { x: number, y: number, z: number }  // multipliers
}
```

### Transform Chain

Every part in vcad has a canonical transform chain: `Primitive -> Scale -> Rotate -> Translate`. This ensures consistent behavior and predictable results.

**Update (2026-07-30):** Mirror is now shipped — `CsgOp::Mirror` plus the `addMirror(partId, plane)` store action.

## UX Details

### Transform Gizmo

Visual 3D handles in the viewport for interactive manipulation:

| Mode | Appearance | Interaction |
|------|------------|-------------|
| Translate | RGB arrows (X/Y/Z) | Drag arrows to move along axis |
| Rotate | RGB rings (X/Y/Z) | Drag rings to rotate around axis |
| Scale | RGB cubes at arrow tips | Drag cubes to scale along axis |

### Property Panel

Precise numeric entry for transform values:

- **Position:** X, Y, Z inputs (mm)
- **Rotation:** X, Y, Z inputs (degrees)
- **Scale:** X, Y, Z inputs (multipliers, default 1.0)

All inputs support scrubbing for quick adjustments.

### Interaction States

| State | Behavior |
|-------|----------|
| Hover gizmo handle | Handle highlights |
| Drag handle | Live preview, value updates in property panel |
| Release | Commits change with undo point |
| Type in property panel | Live preview as you type, Enter commits |

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-ir/src/lib.rs` | `CsgOp::Translate`, `CsgOp::Rotate`, `CsgOp::Scale` types |
| `packages/ir/src/index.ts` | TypeScript mirror of IR types |
| `packages/core/src/stores/document-store.ts` | `setTranslation`, `setRotation`, `setScale` actions |
| `packages/engine/src/evaluate.ts` | Transform evaluation during mesh generation |
| `packages/app/src/components/TransformGizmo.tsx` | 3D gizmo component |
| `packages/app/src/components/PropertyPanel.tsx` | Transform inputs in panel |

### Data Flow

1. User drags gizmo or edits property panel
2. Store action updates IR node (`setTranslation`, `setRotation`, `setScale`)
3. Engine re-evaluates document
4. Viewport renders updated mesh

### Transform3D Type

For assemblies, a combined transform type exists:

```typescript
interface Transform3D {
  translation: Vec3;  // mm
  rotation: Vec3;     // degrees (Euler XYZ)
  scale: Vec3;        // multipliers
}
```

## Tasks

All tasks complete.

- [x] Define `CsgOp::Translate` in IR
- [x] Define `CsgOp::Rotate` in IR
- [x] Define `CsgOp::Scale` in IR
- [x] Implement transform evaluation in engine
- [x] Add `setTranslation`, `setRotation`, `setScale` store actions
- [x] Build transform gizmo component
- [x] Wire property panel inputs
- [x] Integrate undo/redo

## Acceptance Criteria

- [x] Parts can be translated to any position via gizmo or property panel
- [x] Parts can be rotated to any orientation via gizmo or property panel
- [x] Parts can be scaled non-uniformly via gizmo or property panel
- [x] Transform changes are reflected in live preview
- [x] Transform changes create undo points
- [x] Transform values persist in `.vcad` document format

## Future Enhancements

- [x] Mirror operation UI (shipped: `CsgOp::Mirror` + `addMirror` store action)
- [ ] Transform relative to other objects (snap to face, align to edge)
- [ ] Transform multiple selected parts simultaneously
- [ ] Constrain transforms (e.g., lock Y axis during translation)
