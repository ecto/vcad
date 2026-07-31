# Assembly & Joints

Build multi-part assemblies with kinematic joints for mechanical motion simulation.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p0` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Shipped: assembly/joints in the app, FK solver, and physics-backed joints (`create_robot_env`, `gym_*` MCP tools).

## Problem

Complex products require multiple parts that move relative to each other. Without assembly capabilities:

1. Each part must be positioned manually with absolute transforms
2. Kinematic relationships (hinges, sliders, pivots) cannot be expressed
3. Motion simulation requires external tools
4. Design changes ripple into tedious repositioning of dependent parts

Real-world mechanical assemblies need relationships like "this arm rotates around that pin" or "this piston slides along that axis" — not just static geometry.

## Solution

Full assembly system with part reuse, instancing, and kinematic joints.

### Part Definitions

Reusable part templates that define geometry once:

```typescript
interface PartDef {
  id: string;               // Unique identifier
  name?: string;            // Human-readable name
  root: NodeId;             // Root node of geometry DAG
  defaultMaterial?: string; // Material key
}
```

### Part Instances

References to part definitions with per-instance transforms and materials:

```typescript
interface Instance {
  id: string;               // Unique identifier
  partDefId: string;        // Reference to PartDef
  name?: string;            // Instance-specific name
  transform?: Transform3D;  // Override transform
  material?: string;        // Override material
}
```

### Joint Types

Five joint types covering common mechanical connections:

| Joint Type | DOF | Description |
|------------|-----|-------------|
| **Fixed** | 0 | No relative motion, rigid attachment |
| **Revolute** | 1 | Rotation around axis (hinge, pivot) |
| **Slider** | 1 | Translation along axis (piston, rail) |
| **Cylindrical** | 2 | Rotation + translation along same axis |
| **Ball** | 3 | Omnidirectional rotation (ball-and-socket) |

Each joint connects a parent instance to a child instance via anchor points:

```typescript
interface Joint {
  id: string;
  name?: string;
  parentInstanceId: string | null; // null = world-grounded
  childInstanceId: string;
  parentAnchor: Vec3;              // Anchor point on parent
  childAnchor: Vec3;               // Anchor point on child
  kind: JointKind;                 // Type-specific parameters
  state: number;                   // Current angle (deg) or position (mm)
}
```

### Joint Limits

Revolute and Slider joints support optional motion limits:

```typescript
// Revolute with limits
kind: {
  type: "Revolute",
  axis: { x: 0, y: 0, z: 1 },
  limits: [-90, 90]  // Degrees
}

// Slider with limits
kind: {
  type: "Slider",
  axis: { x: 1, y: 0, z: 0 },
  limits: [0, 100]  // Millimeters
}
```

### Forward Kinematics

Solve joint chain to compute world-space poses for all instances:

1. Start from ground instance (fixed in world space)
2. BFS traverse joint tree from ground
3. For each joint, compute child transform based on joint type and state
4. Compose transforms: parent world * joint * child local
5. Return map of instance ID to world transform

## UX Details

### Feature Tree

Assembly elements appear in the feature tree:

```
Document
├── Part Definitions
│   ├── Base Plate
│   └── Arm
├── Instances
│   ├── Base Plate 1 (ground)
│   ├── Arm 1
│   └── Arm 2
└── Joints
    ├── Base-Arm1 Revolute
    └── Arm1-Arm2 Revolute
```

### Property Panel

When a joint is selected, the property panel shows:

| Control | Purpose |
|---------|---------|
| Name | Editable joint name |
| Type | Joint type indicator (read-only) |
| State slider | Adjust current angle/position |
| Limits display | Show min/max if defined |

### Interaction States

| State | Behavior |
|-------|----------|
| Hover instance | Highlight all faces |
| Select instance | Show transform gizmo, property panel |
| Scrub joint slider | Live FK update in viewport |
| Drag instance | Update transform (if no constraining joint) |

### Edge Cases

- **Cyclic joint chains**: Detected and reported as error
- **Missing parent**: Joint becomes world-grounded
- **Deleted instance**: All joints referencing it are also deleted
- **Zero-length axis**: Normalized with fallback to Z axis

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-ir/src/lib.rs` | IR types: `PartDef`, `Instance`, `Joint`, `JointKind` |
| `packages/engine/src/kinematics.ts` | Forward kinematics solver |
| `packages/core/src/stores/document-store.ts` | State management for assembly operations |

### Document Schema

Assembly data stored in `.vcad` document:

```typescript
interface Document {
  // ... existing fields ...
  partDefs?: Record<string, PartDef>;  // Part definitions
  instances?: Instance[];               // Part instances
  joints?: Joint[];                      // Kinematic joints
  groundInstanceId?: string;            // Fixed reference instance
}
```

### Store Operations

```typescript
// Assembly mutations in document-store
createPartDef(partId: string, name?: string): string | null;
createInstance(partDefId: string, name?: string, transform?: Transform3D): string;
addJoint(config: JointConfig): string;
deleteInstance(instanceId: string): void;
deleteJoint(jointId: string): void;
setGroundInstance(instanceId: string): void;
setJointState(jointId: string, state: number, skipUndo?: boolean): void;
renameInstance(instanceId: string, name: string): void;
setInstanceTransform(instanceId: string, transform: Transform3D, skipUndo?: boolean): void;
setInstanceMaterial(instanceId: string, materialKey: string): void;
```

### FK Algorithm

```typescript
// Simplified FK solve
function solveForwardKinematics(doc: Document): Map<string, Transform3D> {
  const results = new Map();
  const jointTree = buildJointTree(doc.joints);

  // Initialize root instances
  for (const instance of getRootInstances(doc)) {
    results.set(instance.id, instance.transform ?? identity());
  }

  // BFS from ground
  const queue = [null, ...rootInstanceIds];
  while (queue.length > 0) {
    const parentId = queue.shift();
    for (const childId of getChildren(jointTree, parentId)) {
      const joint = jointTree.get(childId);
      const parentWorld = results.get(parentId) ?? identity();
      const jointTransform = computeJointTransform(joint);
      const childLocal = instances.get(childId).transform ?? identity();
      results.set(childId, compose(parentWorld, jointTransform, childLocal));
      queue.push(childId);
    }
  }

  return results;
}
```

## Tasks

All tasks complete.

## Acceptance Criteria

- [x] Part definitions can be created from existing parts
- [x] Multiple instances of a part definition can be placed
- [x] Five joint types implemented: Fixed, Revolute, Slider, Cylindrical, Ball
- [x] Revolute and Slider joints support optional limits
- [x] Forward kinematics correctly propagates transforms through joint chain
- [x] Joint state sliders update viewport in real-time
- [x] Deleting an instance removes associated joints
- [x] Ground instance establishes world reference frame
- [x] Assembly data serializes to/from `.vcad` format
- [x] Undo/redo works for all assembly operations

## Future Enhancements

- [ ] Inverse kinematics (IK) solver for goal-directed posing
- [ ] Interference/collision detection between instances
- [ ] Motion studies with keyframe animation
- [ ] Joint sequence export (for manufacturing/assembly instructions)
- [ ] Spring and damper joint properties for dynamics simulation
- [ ] Planar joint type (2 DOF translation)
- [ ] Universal joint type (2 DOF rotation)
