---
name: vcad-assembly
description: Build multi-part assemblies with joints and run physics simulations. Use when the user asks about robot arms, mechanisms, hinges, joints, physics simulation, reinforcement learning environments, or assembly of multiple parts.
---

# vcad Assembly & Physics Simulation

Create assemblies from parts, connect them with joints, and run physics simulations via the vcad MCP tools.

## Assembly Structure

An assembly is defined inside `create_cad_document` alongside the parts:

```json
{
  "parts": [...],
  "assembly": {
    "instances": [...],
    "joints": [...],
    "ground": "base-1"
  }
}
```

### Instances

Each instance places a part in the assembly:

```json
{
  "id": "arm-1",
  "part": "arm",
  "name": "Lower Arm",
  "position": {"x": 0, "y": 0, "z": 50},
  "rotation": {"x": 0, "y": 0, "z": 0}
}
```

- `id` — unique identifier (referenced by joints)
- `part` — name of a part in the `parts` array
- `position` — initial position in mm (optional)
- `rotation` — initial rotation in degrees (optional)

### Joints

Joints connect two instances and define degrees of freedom:

```json
{
  "id": "shoulder",
  "name": "Shoulder Joint",
  "parent": "base-1",
  "child": "arm-1",
  "type": "revolute",
  "axis": "y",
  "parent_anchor": {"x": 0, "y": 0, "z": 50},
  "child_anchor": {"x": 0, "y": 0, "z": 0},
  "limits": [-90, 90],
  "state": 0
}
```

- `parent` — instance ID, or `null` for world/ground
- `child` — instance ID
- `type` — joint type (see below)
- `axis` — `"x"`, `"y"`, `"z"`, or custom `{x, y, z}` vector
- `parent_anchor` / `child_anchor` — connection points in local coordinates
- `limits` — `[min, max]` (degrees for revolute, mm for slider)
- `state` — initial angle or position

### Ground

Set `"ground"` to the instance ID of the fixed/base part. This anchors the kinematic chain.

## Joint Types

| Type | DOF | Description | State Unit |
|------|-----|-------------|------------|
| `fixed` | 0 | Rigid attachment, no motion | n/a |
| `revolute` | 1 | Rotation around axis (hinge) | degrees |
| `slider` | 1 | Translation along axis (piston) | mm |
| `cylindrical` | 2 | Rotation + translation on same axis | degrees |
| `ball` | 3 | Omnidirectional rotation | n/a |

## Physics Simulation Tools

The gym-style tools simulate assembly dynamics using phyz:

| Tool | Purpose |
|------|---------|
| `create_robot_env` | Initialize simulation from assembly document |
| `gym_reset` | Reset to initial state, returns observation |
| `gym_step` | Apply action, advance physics, returns observation + reward + done |
| `gym_observe` | Get current state without stepping |
| `gym_close` | Clean up simulation |

### Simulation Workflow

```
create_cad_document (with assembly)
  → create_robot_env (document, end_effector_ids)
    → gym_reset (env_id)
      → gym_step (env_id, action_type, values) [loop]
        → gym_close (env_id)
```

### create_robot_env

```json
{
  "document": "<IR document from create_cad_document>",
  "end_effector_ids": ["gripper-1"],
  "dt": 0.004167,
  "substeps": 4,
  "max_steps": 1000
}
```

Returns: `env_id`, `num_joints`, `action_dim`, `observation_dim`.

### gym_step

```json
{
  "env_id": "sim_1",
  "action_type": "torque",
  "values": [0.5, -0.3]
}
```

Action types:
- `"torque"` — apply torque in Nm to each joint
- `"position"` — set target position (degrees for revolute, mm for slider)
- `"velocity"` — set target velocity (deg/s or mm/s)

The `values` array must have one entry per joint.

Returns:
- `observation` — joint positions, velocities, end effector poses
- `reward` — scalar reward signal
- `done` — true if episode ended (max steps or termination)

### gym_reset / gym_observe

Both return the observation object:

```json
{
  "joint_positions": [0.0, 0.0],
  "joint_velocities": [0.0, 0.0],
  "end_effector_poses": [
    {"position": {"x": 0, "y": 0, "z": 100}, "orientation": {"x": 0, "y": 0, "z": 0, "w": 1}}
  ]
}
```

## Complete Example: 2-DOF Robot Arm

```json
{
  "parts": [
    {
      "name": "base",
      "primitive": {"type": "cylinder", "radius": 25, "height": 10},
      "material": "steel"
    },
    {
      "name": "arm",
      "primitive": {"type": "cube", "size": {"x": 10, "y": 10, "z": 80}},
      "material": "aluminum"
    }
  ],
  "assembly": {
    "instances": [
      {"id": "base-1", "part": "base"},
      {"id": "lower-arm", "part": "arm", "position": {"x": 0, "y": 0, "z": 10}},
      {"id": "upper-arm", "part": "arm", "position": {"x": 0, "y": 0, "z": 90}}
    ],
    "joints": [
      {
        "id": "shoulder",
        "parent": "base-1",
        "child": "lower-arm",
        "type": "revolute",
        "axis": "y",
        "parent_anchor": {"x": 0, "y": 0, "z": 10},
        "child_anchor": {"x": 5, "y": 5, "z": 0},
        "limits": [-90, 90]
      },
      {
        "id": "elbow",
        "parent": "lower-arm",
        "child": "upper-arm",
        "type": "revolute",
        "axis": "y",
        "parent_anchor": {"x": 5, "y": 5, "z": 80},
        "child_anchor": {"x": 5, "y": 5, "z": 0},
        "limits": [0, 135]
      }
    ],
    "ground": "base-1"
  }
}
```

After creating the document, run a simulation:

1. `create_robot_env` with `end_effector_ids: ["upper-arm"]`
2. `gym_reset` to get initial observation
3. `gym_step` with `action_type: "torque"` and values per joint
4. Repeat step 3 for your control loop
5. `gym_close` when done

## Tips

- Every assembly needs a `ground` instance — this is the fixed reference
- Joint anchors are in the instance's local coordinate frame
- Revolute limits are in degrees, slider limits in mm
- State outside limits is clamped automatically
- Use `position` action type for precise pose control, `torque` for dynamic simulation
- `end_effector_ids` must reference valid instance IDs in the assembly
- Forward kinematics propagates transforms from ground through the joint chain
