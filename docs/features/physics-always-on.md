# Physics Always On

**Score: 89/100** | **Priority: #2**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |

> Verified against repo 2026-07-30. The always-on paradigm described here is not shipped: no physics Web Worker / SharedArrayBuffer loop in `packages/app`, no automatic joint inference, no drop-under-gravity default. What has shipped is on-demand phyz simulation via `crates/vcad-kernel-physics` and the gym MCP tools (`create_robot_env`, `gym_step`, …) — see physics-simulation.md.

## Overview

Physics is the default state of the world—no "run simulation" button required. Drop a part and it falls. Connect two parts and they're jointed. Drag a body and feel the inertia. The CAD environment *is* a physical world.

This inverts the traditional CAD paradigm where physics is an afterthought bolted onto static geometry. In vcad, every solid exists in a physics simulation from the moment of creation.

## Why It Matters

### The Problem with Traditional CAD

Traditional CAD tools treat physics as a separate post-process:

1. Design geometry in a static, weightless void
2. Export to simulation software
3. Set up materials, constraints, contacts
4. Run simulation
5. Discover interference or mechanism issues
6. Return to CAD and iterate

This workflow is fundamentally backwards. Engineers mentally simulate mechanisms while designing—they imagine parts moving, colliding, articulating. Then they verify that mental model hours or days later in a separate tool.

### The vcad Approach

Mechanism design should happen *in* a physical world:

- **Immediate feedback**: See how parts interact the moment they exist
- **Intuitive constraints**: Snap two faces together and get a working joint
- **Natural exploration**: Drag parts to understand range of motion
- **Early collision detection**: Interference is visible immediately, not after export

## Technical Implementation

### Physics Engine

- **phyz** physics engine integrated via `vcad-kernel-physics` crate
- Compiled to WASM for browser execution
- Penalty-based contacts; CCD is not yet implemented in phyz
- Stable simulation up to 240Hz internal step rate

### Architecture

```
┌─────────────────┐     ┌─────────────────┐
│   Main Thread   │     │   Web Worker    │
│                 │     │                 │
│  Three.js       │◄────│  phyz           │
│  React UI       │     │  Physics Step   │
│  User Input     │────►│  Collision Det  │
└─────────────────┘     └─────────────────┘
        │                       │
        └───── SharedArrayBuffer ───────┘
              (transform sync)
```

Physics runs in a dedicated Web Worker to keep UI responsive. Transform data syncs via SharedArrayBuffer for zero-copy updates to the renderer.

### Automatic Joint Inference

When geometric constraints are applied, vcad infers the appropriate physics joint:

| Geometric Constraint | Physics Joint |
|---------------------|---------------|
| Coincident (face-face) | Fixed joint |
| Coincident (axis-axis) | Revolute joint |
| Coincident (point-point) | Ball joint |
| Tangent (cylinder-cylinder) | Prismatic joint |
| Parallel + Distance | Prismatic joint |

Joint inference uses constraint topology analysis with >90% accuracy for common patterns.

### Configuration

Global physics parameters are document-level settings:

```json
{
  "physics": {
    "gravity": [0, 0, -9810],
    "defaultFriction": 0.5,
    "defaultRestitution": 0.3,
    "enabled": true
  }
}
```

Per-part overrides:

```json
{
  "part": {
    "material": "steel",
    "physics": {
      "frozen": false,
      "friction": 0.8,
      "restitution": 0.1
    }
  }
}
```

## User Experience

### Default Behavior

1. **Create a primitive** → It spawns with mass/inertia computed from material density and geometry volume
2. **Place a part** → It drops under gravity and settles on surfaces below
3. **Connect faces** → Appropriate joint is created automatically
4. **Drag a body** → Physics forces applied, connected parts follow via joints

### Freeze Mode

Sometimes precise positioning requires disabling physics:

- **Per-part freeze**: Toggle via context menu or `F` hotkey
- **Global freeze**: Pause all physics via toolbar toggle or `Shift+F`
- Frozen parts render with a subtle ice-blue tint
- Unfreezing re-enables physics from current positions

### Visual Feedback

- **Center of mass indicator**: Small sphere at CoM when part selected
- **Joint visualization**: Axis lines and rotation arcs for revolute joints
- **Contact points**: Highlight where parts touch during collision
- **Velocity vectors**: Optional arrows showing linear/angular velocity

## Success Metrics

| Metric | Target | Rationale |
|--------|--------|-----------|
| Physics step time | <2ms | 60fps with overhead for rendering |
| Typical assembly size | <50 bodies | Covers 90% of mechanism design |
| Joint inference accuracy | >90% | Users shouldn't need to manually fix joints |
| Time to first physics interaction | 0ms | No "enable physics" step |
| Memory overhead | <50MB | Acceptable for browser environment |

### Performance Scaling

| Bodies | Step Time (target) |
|--------|-------------------|
| 10 | <0.5ms |
| 50 | <2ms |
| 100 | <5ms |
| 500 | <20ms |

Large assemblies (>100 bodies) may require selective physics regions or level-of-detail simulation.

## Dependencies

- `vcad-kernel-physics`: phyz integration crate
- `vcad-kernel-physics-wasm`: WASM bindings for browser
- Web Worker support in browser
- SharedArrayBuffer (requires COOP/COEP headers)

## Competitive Advantage

**No CAD tool treats physics as the default state.**

| Tool | Physics Approach |
|------|------------------|
| Fusion 360 | Separate "Motion Study" mode |
| Onshape | No built-in physics |
| SolidWorks | Separate "Motion Analysis" add-in |
| FreeCAD | Assembly3 solver, no real-time physics |
| **vcad** | **Always on, zero configuration** |

This creates a fundamentally different design experience. Mechanisms *work* by default. Interference is discovered immediately. The mental model matches the tool behavior.

## Future Enhancements

- **Soft body physics**: Flexible parts, springs, cables
- **Fluid coupling**: Basic buoyancy and drag forces
- **Motor simulation**: Torque curves, speed limits
- **Physics recording**: Capture and replay motion for presentations
- **Distributed simulation**: Offload heavy assemblies to server
