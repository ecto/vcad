# 10-Second Robot

**Score: 75/100** | **Priority: #15**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |

> Verified against repo 2026-07-30. NL→robot generation is not implemented; however, the physics substrate exists: `create_robot_env`/`gym_*` MCP tools and assembly joints are shipped.

## Overview

From nothing to a simulating robot in 10 seconds. A user types "two-finger gripper on a 3-DOF arm" and the viewport populates with a complete robotic arm and gripper, physics already running, joints highlighted and ready to control.

This feature transforms robot prototyping from a multi-hour CAD exercise into an instant creative act.

## Why It Matters

### Current State of Robot Design

- **Hours before first simulation**: Traditional workflow requires modeling each link, defining joints, setting up physics parameters, configuring constraints—all before seeing the robot move
- **High barrier to entry**: Only experienced CAD users can prototype robots
- **Iteration is slow**: Testing a different arm configuration means repeating the entire process

### What 10-Second Robot Enables

- **Instant prototyping**: Ideas become simulations immediately
- **Lower barrier**: Anyone who can describe a robot can build one
- **Rapid iteration**: Try 10 different configurations in the time it takes to model one
- **Viral demo potential**: "Watch me build a robot in 10 seconds" is shareable content

## Technical Implementation

### Pipeline Architecture

```
Natural Language → Parser → Robot Spec → Assembly Generator → Physics Config → Render
```

### 1. Natural Language Parser

Extracts robot specification from free-form text:

- **Arm type**: DOF count, configuration (serial, SCARA, delta)
- **End effector**: gripper type, tool
- **Base**: fixed, mobile, mounting
- **Modifiers**: size, reach, payload hints

Example parsing:
```
"Two-finger gripper on a 3-DOF arm"
→ {
    arm: { type: "serial", dof: 3 },
    end_effector: { type: "parallel_jaw", fingers: 2 },
    base: { type: "fixed" }  // default
  }
```

### 2. Parametric Primitives Library

Pre-built, fully parametric components:

| Category | Primitives |
|----------|------------|
| Links | Cylindrical, box, L-bracket, custom profile |
| Joints | Revolute, prismatic, spherical, fixed |
| Grippers | Parallel jaw, three-finger, suction, soft/compliant |
| Bases | Fixed mount, wheeled differential, tracked, omni |
| Sensors | Camera mount, force/torque sensor, proximity |

Each primitive exposes parameters:
```typescript
interface Link {
  length: number;      // mm
  diameter: number;    // mm
  mass: number;        // kg (auto-calculated from material)
  material: Material;
}

interface RevoluteJoint {
  axis: Vector3;
  limits: [number, number];  // degrees
  maxTorque: number;         // Nm
  damping: number;
}
```

### 3. Assembly Generator

Converts robot spec into vcad assembly:

1. **Select primitives** from library based on spec
2. **Size appropriately** using heuristics (3-DOF arm → ~400mm reach)
3. **Connect via joints** with proper parent-child relationships
4. **Position in world** at sensible default location

### 4. Physics Auto-Configuration

Sensible defaults derived from geometry:

| Parameter | Default Strategy |
|-----------|------------------|
| Mass | Calculate from volume × material density |
| Inertia | Compute from mesh geometry |
| Joint limits | Standard ranges per joint type |
| Damping | 0.1 × max torque (prevents oscillation) |
| Friction | Material-based lookup |
| Collision | Auto-generate from visual geometry |

### 5. Immediate Simulation

- Physics starts automatically (subtle swaying shows it's live)
- Gravity applied, robot settles into rest pose
- Joint controls appear in sidebar
- End effector position displayed

## Example Flow

```
User: "Two-finger gripper on a 3-DOF arm"

System:
1. Parse: 3-DOF serial arm + 2-finger parallel jaw gripper
2. Generate:
   - Base link (fixed to world)
   - Shoulder joint (revolute, ±180°)
   - Upper arm link (150mm)
   - Elbow joint (revolute, -135° to +135°)
   - Forearm link (120mm)
   - Wrist joint (revolute, ±180°)
   - Gripper mount
   - Two finger links with prismatic joints
3. Configure:
   - Joint torque limits from link masses
   - Gripper force limit: 10N default
   - Collision meshes for all links
4. Render:
   - Robot appears at world origin
   - Physics running, arm settles
   - Joints highlighted with rotation indicators
5. Ready:
   - Control panel shows 4 sliders (3 arm + 1 gripper)
   - End effector pose displayed
   - "Edit" button to modify parameters
```

**Total time: <10 seconds**

## Primitives Library

### Arms

| Type | Description | Use Case |
|------|-------------|----------|
| 2-DOF | Shoulder + elbow, planar motion | Simple pick-and-place |
| 3-DOF | + wrist rotation | General manipulation |
| 6-DOF | Full orientation control | Precision tasks |
| SCARA | 4-DOF, fast horizontal motion | Assembly lines |
| Delta | Parallel, high speed | Pick-and-place |

### Grippers

| Type | Description | Use Case |
|------|-------------|----------|
| Parallel jaw | 2 fingers, linear motion | General grasping |
| Three-finger | Adaptive, centered grasp | Cylindrical objects |
| Suction | Vacuum-based | Flat surfaces |
| Soft | Compliant fingers | Delicate objects |
| Magnetic | Electromagnet | Ferrous materials |

### Bases

| Type | Description | Use Case |
|------|-------------|----------|
| Fixed | Bolted to world | Stationary tasks |
| Wheeled (diff) | Differential drive | Mobile manipulation |
| Wheeled (omni) | Omnidirectional | Agile navigation |
| Tracked | Tank treads | Rough terrain |
| Legged | 4/6 legs | Unstructured environments |

### Sensors

| Type | Attachment | Output |
|------|------------|--------|
| Camera mount | End effector or base | RGB/depth |
| Force/torque | Wrist | 6-axis wrench |
| Proximity | Gripper fingers | Distance |
| Joint encoder | Each joint | Position/velocity |

## User Interface

### Input Methods

1. **Text prompt**: Natural language in command palette
2. **Quick buttons**: "3-DOF Arm", "6-DOF Arm", "Mobile Base"
3. **Template gallery**: Visual selection of common robots

### Post-Generation Controls

- **Joint sliders**: Direct control of each DOF
- **End effector target**: Drag to set IK goal
- **Parameter panel**: Edit dimensions, limits, materials
- **Explode view**: See assembly structure

### Editing Generated Robots

Generated robots are standard vcad assemblies:

- Full feature tree visible
- Each link/joint individually selectable
- Parameters editable via property panel
- Can add/remove components
- Save as template for reuse

## Success Metrics

| Metric | Target |
|--------|--------|
| Time to simulation | <10 seconds |
| Parse success rate | >90% for simple descriptions |
| Physics stability | No explosions on generation |
| Editability | 100% of generated geometry modifiable |
| User satisfaction | "It just works" feeling |

### Validation Approach

1. **Corpus testing**: 100 natural language robot descriptions
2. **Time trials**: Measure end-to-end latency
3. **User studies**: Can novices generate and control robots?
4. **Comparison**: Time vs. traditional CAD workflow

## Competitive Advantage

| Tool | Robot Creation Time | Simulation | Editable |
|------|---------------------|------------|----------|
| Fusion 360 | Hours | No (external) | Yes |
| Onshape | Hours | No (external) | Yes |
| Gazebo | 30+ min (URDF) | Yes | XML only |
| Isaac Sim | 30+ min | Yes | Limited |
| **vcad** | **<10 seconds** | **Yes** | **Yes** |

**No existing tool offers instant, natural-language robot generation with integrated simulation and full editability.**

## Implementation Phases

### Phase 1: Core Pipeline
- Text parser for arm + gripper descriptions
- Basic primitives library (serial arms, parallel jaw)
- Assembly generator
- Physics auto-configuration

### Phase 2: Expanded Library
- More arm types (SCARA, delta)
- More grippers (suction, soft)
- Mobile bases
- Sensor mounts

### Phase 3: Intelligence
- Learn from user edits to improve defaults
- Suggest modifications based on task description
- Auto-generate control policies

## Dependencies

- Natural language parsing (LLM or rule-based)
- Parametric primitive library
- Assembly system with joints
- Physics simulation (already exists via gym interface)
- Real-time rendering (already exists)

## Risks

| Risk | Mitigation |
|------|------------|
| Ambiguous descriptions | Ask clarifying questions, show alternatives |
| Physics instability | Conservative defaults, auto-tuning |
| User expects too much | Clear capability documentation |
| Performance on complex robots | Progressive loading, LOD |

## Future Extensions

- **Voice input**: "Hey vcad, build me a robot arm"
- **Sketch input**: Draw rough robot shape, system interprets
- **Task-driven generation**: "Robot to stack blocks" → appropriate design
- **Multi-robot**: "Two arms on a mobile base"
- **Export to hardware**: Generate BOM, 3D print files, motor specs
