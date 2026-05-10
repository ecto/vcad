# Never Leave the Canvas

**Score: 86/100** | **Priority: #4**

## Overview

One infinite canvas where geometry, physics, code, and conversation all live together. No modes, no exports, no "save and open in another tool." The entire design-simulate-iterate loop happens in a single viewport.

## Why It Matters

### The Fragmented Status Quo

A typical robotics/mechanical design workflow:

1. Model geometry in Fusion 360 or Onshape
2. Export URDF/SDF for Gazebo simulation
3. Open VS Code to write control logic
4. Run simulation, discover collision issues
5. Back to CAD to fix geometry
6. Re-export, re-simulate, repeat

Each context switch breaks flow state. Information scatters across disconnected files. The mental model fragments.

### The Cost of Switching

- **Cognitive load**: Rebuilding mental context takes 15-25 minutes per switch
- **Data drift**: Exported files become stale; manual sync required
- **Lost insights**: Physics behavior observed in sim never makes it back to CAD annotations
- **Collaboration friction**: "Check the Gazebo world file" vs "Look at line 47 in the controller"

## Technical Implementation

### Unified Viewport Architecture

A single Three.js viewport renders multiple synchronized layers:

| Layer | Content | Interaction |
|-------|---------|-------------|
| Geometry | BRep meshes, wireframes | Select, transform, sketch |
| Physics | Force vectors, contact points, trajectories | Hover to inspect values |
| Annotations | Dimensions, notes, AI suggestions | Click to edit or accept |
| Code | Inline editors attached to geometry | Execute with Cmd+Enter |

Layers toggle independently. Physics viz can overlay geometry during simulation playback.

### Inline Code Execution

Code blocks attach directly to parts or assemblies:

```python
# Attached to "gripper_assembly"
def close_gripper(force_limit=10.0):
    joint = self.joints["finger_left"]
    joint.set_target_position(-15, force_limit)
    joint = self.joints["finger_right"]
    joint.set_target_position(15, force_limit)
```

Execution happens against the live model. Results render immediately—joint moves, forces display, collisions highlight.

### Integrated Chat Panel

The chat panel shares full context with the viewport:

- Current selection (parts, joints, sketches)
- Document history (what changed, when)
- Active physics state (positions, velocities, forces)
- Code block outputs and errors

When AI responds, it can:
- Highlight geometry ("this face has thin wall issues")
- Suggest modifications with preview
- Insert executable code blocks
- Add annotations directly to the model

### Timeline Integration

A single timeline scrubber controls:

- Undo/redo history (design changes)
- Physics simulation playback
- Animation keyframes
- Version comparison ("show state from 2 hours ago")

Scrubbing backwards through a simulation shows both the physics state and the geometry state at that moment.

### Adaptive Property Panel

The property panel morphs based on selection:

| Selection | Panel Contents |
|-----------|----------------|
| Part | Dimensions, material, mass properties |
| Joint | Limits, damping, current state, control mode |
| Code block | Language, execution history, attached geometry |
| Annotation | Text, style, linked geometry |
| Physics entity | Force magnitude, contact normal, friction |

## User Experience

### Unified Workflow Example

1. **Design**: Create gripper geometry with sketch → extrude → pattern
2. **Assemble**: Add joints, set limits—see range-of-motion preview in viewport
3. **Code**: Write control function in inline block attached to assembly
4. **Simulate**: Press play, watch gripper close, see contact forces render
5. **Iterate**: Notice collision, click face, type fix, see result
6. **Ask AI**: "Why is the left finger slipping?" → AI highlights contact patch, suggests material change, previews improvement

No exports. No app switching. No file management.

### Visual Information Density

The viewport shows everything relevant:

```
┌─────────────────────────────────────────────────────────────────┐
│  [Geometry]     [Physics]     [Annotations]      │   Chat      │
│  ┌───────────────────────────────────────────┐   │             │
│  │                                           │   │  > Why is   │
│  │     ┌─────────┐                           │   │    grip     │
│  │     │ gripper │←── force: 8.2N            │   │    weak?    │
│  │     │   ○━━○  │                           │   │             │
│  │     └────┼────┘                           │   │  The contact│
│  │          │ ← "thin wall warning"          │   │  area is    │
│  │          ▼                                │   │  only 12mm² │
│  │     [workpiece]                           │   │  [highlight]│
│  │                                           │   │             │
│  └───────────────────────────────────────────┘   │             │
│  ══════════════════●═══════════════════════════  │             │
│  [timeline: 0.0s ──────── 2.3s ──────── 5.0s]    │ [properties]│
└─────────────────────────────────────────────────────────────────┘
```

Toggle physics layer off for clean screenshots. Toggle annotations on for documentation. Everything in one place.

## UI Layout

```
┌─────────────────────────────────────────────────────────────────┐
│ toolbar                                                         │
├───────────────────────────────────────────────┬─────────────────┤
│                                               │                 │
│                                               │                 │
│           viewport                            │     chat        │
│   (geometry + physics + annotations)          │                 │
│                                               │                 │
│                                               │                 │
├───────────────────────────────────────────────┼─────────────────┤
│          timeline / scrubber                  │   properties    │
└───────────────────────────────────────────────┴─────────────────┘
```

- **Viewport**: Dominates screen, layers toggle via toolbar
- **Chat**: Persistent sidebar, collapsible, context-aware
- **Timeline**: Horizontal strip, unified for history + simulation
- **Properties**: Adapts to selection, dockable anywhere

## Success Metrics

| Metric | Target |
|--------|--------|
| Exports required for design-sim-iterate loop | **0** |
| Apps open during typical workflow | **1** |
| Time to see physics result after geometry change | **< 2 seconds** |
| AI context completeness | **100%** (geometry + physics + history) |
| Tab switches per design session | **0** |

### Qualitative Goals

- Designer never asks "where did I save that?"
- Simulation insights (forces, collisions) visible in design context
- AI suggestions appear where they're relevant, not in a separate window
- History is visual, not a list of filenames

## Competitive Advantage

### Current Tool Landscape

| Tool | Strength | Limitation |
|------|----------|------------|
| Fusion 360 | Geometry | No physics, weak scripting |
| Onshape | Collaboration | No simulation |
| Gazebo | Physics | No CAD, XML hell |
| VS Code | Code | No geometry context |
| Blender | Everything | Steep learning curve, not CAD |

Every existing tool forces you out for some part of the workflow.

### vcad Differentiation

- **Single source of truth**: One document, one viewport, one history
- **AI-native**: Chat has full context because everything is in one place
- **Physics-integrated**: Simulation isn't a separate export; it's a viewport layer
- **Code-first optional**: Write scripts or don't—same canvas either way

The moat is integration depth. Competitors would need to rebuild from scratch to match this architecture.

## Implementation Notes

### Dependencies

- Unified document format (already exists in vcad)
- Physics engine integration (planned: phyz)
- Code execution sandbox (WASM-based for security)
- Chat context protocol (extend current MCP)

### Phased Rollout

1. **Phase 1**: Geometry + annotations + chat (current state)
2. **Phase 2**: Timeline scrubber for undo/redo history
3. **Phase 3**: Physics layer with force visualization
4. **Phase 4**: Inline code blocks with execution
5. **Phase 5**: Unified simulation playback in timeline

### Risks

- Performance with all layers active—mitigate with LOD and layer culling
- Code execution security—sandboxed WASM environment required
- Information overload—sensible defaults, easy layer toggles
