# CAD ↔ Physics Round-Trip

**Score: 87/100** | **Priority: #3**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. The CAD→physics direction is shipped (`crates/vcad-kernel-physics`, `packages/engine` PhysicsEnv, `create_robot_env`/`gym_*` MCP tools build sims directly from the parametric assembly); the live edit-with-preserved-sim-state round-trip described below is not verified as implemented.

## Overview

Parametric geometry stays live through simulation. Change a dimension, and the physics world updates immediately. No re-export, no mesh reimport, no manual synchronization.

The parametric DAG and physics world are fundamentally synchronized:
- Geometry changes propagate to collision shapes automatically
- Joint anchors update when connected geometry changes
- Material properties flow from CAD definitions to physics simulation
- Physics state is preserved through edits when topologically possible

## Why It Matters

### The Traditional Workflow is Broken

```
CAD → Export STL → Import to Sim → Find Problem → Back to CAD → Re-export → Repeat
```

Every iteration requires:
1. Manual export from CAD tool
2. Manual import into simulation
3. Re-configuration of physics properties
4. Loss of parametric intelligence (mesh is "dead")

### NVIDIA Isaac and Similar Tools

Even state-of-the-art robotics simulators like NVIDIA Isaac import dead meshes. Once geometry enters the simulation:
- Parametric relationships are lost
- Changing a dimension means starting over
- No feedback loop between design and simulation

### vcad Approach

```
Change dimension → See physics effect immediately
```

The parametric model IS the physics model. They share truth.

## Technical Implementation

### Unified Document Model

The vcad document contains both:
- **Parametric DAG**: Operations, parameters, dependencies
- **Physics State**: Bodies, joints, velocities, forces

```typescript
interface VcadDocument {
  // Parametric geometry
  parts: Part[];
  operations: Operation[];
  parameters: Parameter[];

  // Physics world
  physics: {
    bodies: PhysicsBody[];
    joints: PhysicsJoint[];
    state: SimulationState;
  };
}
```

### Change Propagation

When a parameter changes:

1. **DAG Evaluation**: Affected operations re-evaluate
2. **Geometry Delta**: System identifies which parts changed
3. **Incremental Update**: Only changed collision shapes rebuild
4. **Anchor Recomputation**: Joint attachment points update
5. **Physics Continuation**: Simulation continues from current state

```
Parameter Change
      ↓
DAG Re-evaluation (affected nodes only)
      ↓
Geometry Delta Detection
      ↓
Collision Shape Update (incremental)
      ↓
Joint Anchor Update
      ↓
Physics Continuation
```

### Incremental Collision Shapes

Full mesh regeneration is expensive. vcad uses:
- **Primitive tracking**: Know when a collision shape is still a primitive
- **Convex decomposition caching**: Reuse decomposition when topology unchanged
- **Partial updates**: Only regenerate meshes for changed bodies

### Joint Anchor Synchronization

When geometry changes, joint anchors must update:
- Anchors defined relative to part geometry
- Part bounding box or feature references
- Automatic recomputation maintains mechanical relationships

### Material Property Flow

```
CAD Material Definition
    ├── Density → Mass calculation
    ├── Friction coefficients → Contact model
    └── Restitution → Collision response
```

## User Experience

### Instant Iteration

**Question**: "What if the gripper fingers were 2mm wider?"

**Traditional workflow**: Export, import, configure, simulate, export, modify, export, import...

**vcad workflow**: Change the parameter. See the result. Sub-50ms update.

### Design Optimization Loops

Iterate without friction:
1. Observe physics behavior
2. Hypothesize improvement
3. Change parameter
4. See result immediately
5. Repeat

No context switching. No file management. No lost state.

### Collision Visibility During Design

Problems appear as you create them:
- Interference detected in real-time
- Motion paths validated continuously
- Clearances checked with actual dynamics

## Use Cases

### Iterative Mechanism Design

Design a linkage, see it move, adjust dimensions, see improved motion—all in one continuous session.

### Clearance Checking with Motion

Not just static interference: simulate full range of motion while adjusting geometry. Find the sweep volume issues before manufacturing.

### Mass/Inertia Optimization

Change wall thickness, see center of mass shift, observe stability change. Optimize weight distribution interactively.

### Generative Design with Physics Constraints

Automated optimization can:
1. Propose geometry change
2. Evaluate physics behavior
3. Score result
4. Iterate

Without round-trip overhead, evaluation is fast enough for serious optimization.

## Success Metrics

| Metric | Target |
|--------|--------|
| Geometry change → physics update | <50ms |
| Manual re-export required | Never |
| Physics state preservation | When topology unchanged |
| Joint anchor update accuracy | Exact (relative definitions) |

## Competitive Advantage

**No existing tool maintains parametric ↔ physics synchronization.**

- Fusion 360: Simulation is separate, requires export
- SolidWorks: Motion study disconnected from parametric model
- Onshape: No integrated physics
- Isaac Sim: Imports dead meshes
- Gazebo: URDF/SDF are static definitions

vcad treats parametric CAD and physics simulation as one unified system. The geometry you design is the geometry that simulates—always in sync, always live.

## Related Features

- [MCP Tools](../mcp.md): AI agents can drive both parametric changes and physics queries
- [Assembly Joints](../assemblies.md): Joint definitions that physics respects
- [Material System](../materials.md): Properties that flow to simulation
