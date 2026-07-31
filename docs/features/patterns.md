# Patterns

Repeat geometry in linear or circular arrays.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | n/a |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30.

## Problem

Creating many copies of features manually is tedious and error-prone:

1. User creates a hole for a bolt pattern
2. Copy, paste, translate to next position
3. Repeat for each hole (6+ times for a typical flange)
4. Changing the pattern requires editing each copy individually
5. No parametric relationship between copies

This workflow doesn't scale. A 10-bolt flange takes 10x the work of a single hole.

## Solution

Two pattern types that create parametric arrays from a single feature:

### Linear Pattern

Repeat geometry along a direction vector with uniform spacing.

```
Original  →  [●]  [●]  [●]  [●]  [●]
              ←─spacing─→
              count = 5
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `child` | NodeId | Feature to pattern |
| `direction` | Vec3 | Direction vector (normalized internally) |
| `count` | u32 | Number of copies including original |
| `spacing` | f64 | Distance between copies |

### Circular Pattern

Repeat geometry around an axis with uniform angular spacing.

```
        [●]
      /     \
    [●]     [●]
      \     /
        [●]

    count = 4
    angle = 360°
```

**Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `child` | NodeId | Feature to pattern |
| `axis_origin` | Vec3 | Point on rotation axis |
| `axis_dir` | Vec3 | Axis direction vector |
| `count` | u32 | Number of copies including original |
| `angle_deg` | f64 | Total angle span in degrees |

**Not included:** Variable spacing, pattern along path, skip instances (see Future).

## UX Details

### Creating Patterns

1. Select feature in tree
2. Right-click > Pattern > Linear or Circular
3. Dialog shows pattern parameters with live preview
4. Arrows/handles in viewport for direction/axis adjustment

### Editing Patterns

- Click pattern in tree to select all instances
- Edit parameters in property panel
- Changes update all instances simultaneously

### Visual Feedback

| State | Behavior |
|-------|----------|
| Preview | Ghost copies at 50% opacity while editing parameters |
| Selected | All pattern instances highlight together |
| Hover | Show pattern direction/axis indicator |

## Implementation

### IR Types

```rust
// crates/vcad-ir/src/lib.rs

/// Linear pattern — repeat geometry along a direction.
LinearPattern {
    /// Child node to pattern.
    child: NodeId,
    /// Direction vector (will be normalized).
    direction: Vec3,
    /// Number of copies (including original).
    count: u32,
    /// Spacing between copies along direction.
    spacing: f64,
},

/// Circular pattern — repeat geometry around an axis.
CircularPattern {
    /// Child node to pattern.
    child: NodeId,
    /// A point on the rotation axis.
    axis_origin: Vec3,
    /// Direction of the rotation axis.
    axis_dir: Vec3,
    /// Number of copies (including original).
    count: u32,
    /// Total angle span in degrees.
    angle_deg: f64,
},
```

### Files

| File | Role |
|------|------|
| `crates/vcad-ir/src/lib.rs` | `CsgOp::LinearPattern`, `CsgOp::CircularPattern` |
| `packages/ir/src/index.ts` | TypeScript IR types |
| `packages/engine/src/evaluate.ts` | Pattern evaluation logic |

### Algorithm

**Linear Pattern:**
1. Evaluate child node to get base solid
2. For i in 1..count: translate base by `direction * spacing * i`
3. Union all translated copies with original

**Circular Pattern:**
1. Evaluate child node to get base solid
2. Compute angle step: `angle_deg / count`
3. For i in 1..count: rotate base around axis by `step * i`
4. Union all rotated copies with original

## Tasks

- [x] Add `LinearPattern` variant to `CsgOp` enum
- [x] Add `CircularPattern` variant to `CsgOp` enum
- [x] Mirror types in TypeScript IR package
- [x] Implement linear pattern evaluation
- [x] Implement circular pattern evaluation
- [x] Add pattern UI in app

## Acceptance Criteria

- [x] Linear pattern creates copies along specified direction
- [x] Circular pattern creates copies around specified axis
- [x] Count parameter includes original (count=1 means just original)
- [x] Patterns are parametric (editing updates all instances)
- [x] Works with any child feature (primitives, booleans, extrudes, etc.)
- [x] Patterns can be nested (pattern of a pattern)

## Future Enhancements

- [ ] Pattern along path (follow curve or edge)
- [ ] Mirror pattern (reflect across plane)
- [ ] Variable spacing (different distances between copies)
- [ ] Skip instances (exclude specific pattern indices)
- [ ] Bidirectional linear pattern (extend in both directions)
- [ ] Pattern table (explicit list of positions/angles)
