# Sketch Mode

Draw constrained 2D profiles for 3D operations like extrude, revolve, sweep, and loft.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `n/a` |
| Priority | `p0` |
| Effort | `n/a` (complete) |

> Verified against repo 2026-07-30. Kernel LM solver in `crates/vcad-kernel-constraints`; `packages/core/src/stores/sketch-store.ts` delegates solving to the kernel via `solveSketchSegments` (WASM).

## Problem

3D modeling operations need 2D input profiles:

1. **Extrude** requires a closed 2D shape to push into 3D
2. **Revolve** requires a profile to spin around an axis
3. **Sweep** requires a profile to follow a path
4. **Loft** requires multiple profiles to blend between

Without a proper sketch system, users would need to manually define profile coordinates or be limited to primitive shapes. Professional CAD workflows demand:

- Precise control over profile geometry
- Geometric constraints (horizontal, parallel, tangent)
- Dimensional constraints (lengths, angles, radii)
- Real-time feedback on constraint status

## Solution

Full 2D sketch system with drawing tools, multiple sketch planes, and a constraint solver.

### Drawing Tools

| Tool | Description | Usage |
|------|-------------|-------|
| Line | Draw connected line segments | Click start, click end, continue or double-click to finish |
| Rectangle | Draw axis-aligned rectangles | Click corner, drag to opposite corner |
| Circle | Draw circles from center | Click center, drag to set radius |
| Arc | Draw circular arcs | Click start, click end, click point on arc |

### Sketch Planes

Users can sketch on:

- **Standard planes:** XY, XZ, YZ (origin-centered)
- **Face selection:** Click any planar face on existing geometry
- **Custom plane:** Define by origin point and normal vector

When entering sketch mode, the camera automatically swings to face the sketch plane for intuitive 2D drawing.

### Constraint Solver

Levenberg-Marquardt solver with adaptive damping:

- **Algorithm:** Iteratively minimize sum of squared constraint errors
- **Damping:** Adaptive lambda (starts at 1e-3, adjusts based on step acceptance)
- **Convergence:** Tolerance of 1e-10 for residual norm
- **Max iterations:** 100 (configurable)

The solver handles:
- Singular matrices (increase damping)
- Over-constrained systems (reports conflict)
- Under-constrained systems (allows free movement)

## UX Details

### Constraint Types

**Geometric Constraints** (no explicit dimension):

| Constraint | Description | Selection |
|------------|-------------|-----------|
| Horizontal | Line parallel to X axis | 1 line |
| Vertical | Line parallel to Y axis | 1 line |
| Coincident | Two points at same location | 2 points |
| Parallel | Two lines same direction | 2 lines |
| Perpendicular | Two lines at 90 degrees | 2 lines |
| Tangent | Line tangent to arc/circle | 1 line + 1 curve |

**Dimensional Constraints** (explicit values):

| Constraint | Description | Selection |
|------------|-------------|-----------|
| Distance | Distance between two points | 2 points |
| Length | Line segment length | 1 line |
| Radius | Circle/arc radius | 1 circle/arc |
| Angle | Angle between two lines | 2 lines |
| Equal Length | Two lines same length | 2 lines |
| Fixed | Point locked at coordinates | 1 point |

### Constraint Status Indicator

The UI displays sketch health:

| Status | Color | Meaning |
|--------|-------|---------|
| Under-constrained | Yellow | Geometry can still move freely |
| Fully constrained | Green | All degrees of freedom locked |
| Over-constrained | Red | Conflicting constraints, cannot solve |

### Interaction Flow

1. **Enter sketch mode:** Click plane gizmo or select face
2. **Camera swings** to face the sketch plane
3. **Draw geometry** using toolbar tools
4. **Apply constraints** by selecting entities and clicking constraint buttons
5. **Solver runs** automatically after each constraint
6. **Exit sketch mode:** Press Escape or click checkmark
7. **Apply 3D operation** (extrude, revolve, etc.) using the profile

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| L | Line tool |
| R | Rectangle tool |
| C | Circle tool |
| H | Apply horizontal constraint |
| V | Apply vertical constraint |
| Escape | Exit sketch mode (prompts if unsaved) |
| Enter | Finish current shape |

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-sketch/src/lib.rs` | Sketch profile types, extrude/revolve operations |
| `crates/vcad-kernel-sketch/src/profile.rs` | `SketchProfile` and `SketchSegment` types |
| `crates/vcad-kernel-sketch/src/extrude.rs` | Extrude profile into solid |
| `crates/vcad-kernel-sketch/src/revolve.rs` | Revolve profile around axis |
| `crates/vcad-kernel-constraints/src/lib.rs` | Constraint solver exports |
| `crates/vcad-kernel-constraints/src/solver.rs` | Levenberg-Marquardt solver |
| `crates/vcad-kernel-constraints/src/constraint.rs` | Constraint type definitions |
| `crates/vcad-kernel-constraints/src/jacobian.rs` | Jacobian matrix computation |
| `crates/vcad-kernel-constraints/src/residual.rs` | Error function computation |
| `packages/core/src/stores/sketch-store.ts` | UI state for sketch mode |

### Data Structures

```typescript
// sketch-store.ts
interface SketchState {
  active: boolean;
  plane: SketchPlane;              // "XY" | "XZ" | "YZ" | CustomPlane
  origin: Vec3;
  segments: SketchSegment2D[];     // Lines, arcs, circles
  constraints: SketchConstraint[];
  tool: "line" | "rectangle" | "circle";
  constraintTool: ConstraintTool;
  selectedSegments: number[];
  solved: boolean;
  constraintStatus: "under" | "solved" | "error";
}
```

```rust
// constraint.rs
pub enum Constraint {
    // Geometric
    Coincident { point_a: EntityRef, point_b: EntityRef },
    Horizontal { line: EntityId },
    Vertical { line: EntityId },
    Parallel { line_a: EntityId, line_b: EntityId },
    Perpendicular { line_a: EntityId, line_b: EntityId },
    Tangent { line: EntityId, curve: EntityId, at_point: EntityRef },
    EqualLength { line_a: EntityId, line_b: EntityId },
    // Dimensional
    Distance { point_a: EntityRef, point_b: EntityRef, distance: f64 },
    Length { line: EntityId, length: f64 },
    Radius { circle: EntityId, radius: f64 },
    Angle { line_a: EntityId, line_b: EntityId, angle_rad: f64 },
    Fixed { point: EntityRef, x: f64, y: f64 },
}
```

### Solver Algorithm

```
1. Compute Jacobian matrix J (partial derivatives of errors w.r.t. parameters)
2. Compute residual vector r (current constraint errors)
3. Form damped normal equations: (J'J + lambda*I) * delta = -J'r
4. Solve for step direction delta using LU decomposition
5. If new error < old error:
   - Accept step, decrease lambda (trust linear model more)
6. Else:
   - Reject step, increase lambda (be more conservative)
7. Repeat until converged or max iterations
```

## Tasks

All tasks complete:

- [x] Implement sketch profile types (`xs`)
- [x] Implement extrude operation (`s`)
- [x] Implement revolve operation (`s`)
- [x] Implement constraint types (`s`)
- [x] Implement Levenberg-Marquardt solver (`m`)
- [x] Implement Jacobian computation (`m`)
- [x] Create sketch store for UI state (`s`)
- [x] Implement drawing tools (line, rectangle, circle) (`m`)
- [x] Implement constraint application UI (`s`)
- [x] Implement face selection for sketch planes (`s`)
- [x] Add camera swing on sketch entry (`xs`)
- [x] Add constraint status indicator (`xs`)
- [x] Add keyboard shortcuts (`xs`)

## Acceptance Criteria

- [x] Can enter sketch mode on XY, XZ, YZ planes
- [x] Can enter sketch mode on any planar face
- [x] Camera automatically faces the sketch plane
- [x] Can draw lines, rectangles, and circles
- [x] Can apply horizontal and vertical constraints
- [x] Can apply coincident, parallel, and perpendicular constraints
- [x] Can apply tangent constraints to curves
- [x] Can set distance, length, radius, and angle dimensions
- [x] Can fix points at specific coordinates
- [x] Constraint solver converges for valid systems
- [x] Over-constrained systems show error state
- [x] Sketch profiles can be used for extrude operations
- [x] Sketch profiles can be used for revolve operations
- [x] Undo/redo works within sketch mode

## Future Enhancements

- [ ] Spline curves (B-spline, Bezier) for freeform profiles
- [ ] Offset operation (create parallel profile at distance)
- [ ] Trim/extend tools for editing intersecting geometry
- [ ] Construction geometry (reference lines that don't extrude)
- [ ] Mirror constraint (symmetric about axis)
- [ ] Pattern constraints (linear/circular arrays)
- [ ] Dimension display directly on geometry
- [ ] Auto-constraint inference while drawing
- [ ] Profile validation (closed, non-intersecting)
- [ ] Multi-profile sketches for complex extrusions
