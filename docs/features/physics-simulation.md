# Physics Simulation & Gym

phyz-based articulated rigid body physics for robotics simulation and RL policy training.

## Status

| Field | Value |
|-------|-------|
| State | `planned` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `l` (3-4 weeks) |
| Target | Post-cad0 |
| Depends On | URDF support |

**Note:** Wait for user demand signal before starting. cad0 milestone is higher priority.

## Problem

Robot designers need to simulate physics and train reinforcement learning policies for their mechanical designs. Currently this requires:

1. **Multiple disconnected tools** — Design in CAD, export mesh, import to MuJoCo/Isaac Sim, manually recreate joint constraints
2. **No parametric feedback loop** — Change a link length in CAD, repeat entire export/import process
3. **Expensive proprietary solutions** — NVIDIA Isaac Sim requires RTX GPU; MuJoCo Pro was commercial until recently
4. **No AI-native integration** — Training scripts can't query or modify CAD parameters programmatically

Robot designers make 50+ design iterations. Each iteration involving physics requires manual re-export and scene reconstruction.

## Solution

Integrated physics simulation with gym-style interface for RL training, all within vcad:

```
vcad Document → BRep → Collision Shapes → phyz Simulation
       ↑                                        ↓
    Parameters ←── MCP Tools ←── Policy Actions/Observations
```

### Core Components

**1. Physics Engine Integration**
- phyz for articulated rigid body dynamics (Featherstone ABA, penalty contacts, joints)
- Automatic collision shape generation from BRep geometry
- Joint mapping from vcad assembly joints to phyz joint types

**2. Collision Shapes from BRep**
- **Convex hull** — Default for performance (VHACD decomposition for concave shapes)
- **Trimesh** — Exact collision when convex is insufficient
- **Primitives** — Box/cylinder/sphere primitives bypass mesh generation

**3. Gym-Style Interface**
- `Observation` — Joint positions, velocities, contact forces, end-effector poses
- `Action` — Joint torques or position targets
- `step(action) → (observation, reward, done, info)` — Standard gym API
- `reset() → observation` — Reset to initial state

**4. MCP Tools for AI Training**
- `create_simulation` — Initialize physics from vcad document
- `step_simulation` — Advance physics, return observations
- `reset_simulation` — Reset to initial state
- `set_action` — Apply joint torques or targets

### Example: Train Panda Arm Reach Policy

```typescript
// Via MCP from training script
const sim = await mcpClient.call('create_simulation', {
  document: 'panda.vcad',
  timestep: 0.001,
  gravity: [0, 0, -9.81]
});

// Training loop
for (let episode = 0; episode < 10000; episode++) {
  let obs = await mcpClient.call('reset_simulation', { id: sim.id });

  while (!done) {
    const action = policy.act(obs);
    const result = await mcpClient.call('step_simulation', {
      id: sim.id,
      action: action,
      steps: 10  // 10 substeps at 1kHz = 100Hz control
    });
    obs = result.observation;
    done = result.done;
  }
}
```

**Not included:** Soft body simulation, fluid dynamics, continuous collision detection.

## UX Details

### App Integration

| Mode | Behavior |
|------|----------|
| **Simulation Mode** | Sidebar toggle switches from design to simulation |
| **Play/Pause** | Toolbar buttons control physics stepping |
| **Time Scrubber** | Scrub through simulation recording |
| **Visualization** | Contact points, forces, trajectories overlaid on viewport |

### Interaction States

| State | Behavior |
|-------|----------|
| Idle | Physics paused, model editable |
| Running | Physics active, model locked, real-time viz |
| Recording | Physics active, states saved for playback |
| Playback | Scrub through recorded simulation |

### Visualization Options

- Contact points (red spheres at contact locations)
- Contact forces (arrows scaled by magnitude)
- Joint torques (arc arrows around joint axes)
- Trajectories (trail of end-effector path)
- Collision shapes (wireframe overlay)

### Error Handling

| Condition | Response |
|-----------|----------|
| Invalid collision geometry | Warning toast, skip problematic face |
| Simulation instability | Auto-reduce timestep, warn user |
| Joint limit exceeded | Clamp + visual indicator |
| No joints found | Error: "Document has no joints to simulate" |

## Implementation

### Files to Create

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-physics/src/lib.rs` | Crate entry, phyz integration |
| `crates/vcad-kernel-physics/src/collision.rs` | BRep → collision shape conversion |
| `crates/vcad-kernel-physics/src/joints.rs` | vcad joint → phyz joint mapping |
| `crates/vcad-kernel-physics/src/gym.rs` | Observation/action types, step/reset |
| `crates/vcad-kernel-physics/src/state.rs` | Simulation state management |
| `crates/vcad-kernel-wasm/src/physics.rs` | WASM bindings for browser |
| `packages/core/src/stores/simulation-store.ts` | Simulation state (running, recording, etc.) |
| `packages/engine/src/physics.ts` | WASM wrapper, TypeScript API |
| `packages/app/src/components/SimulationMode.tsx` | Simulation UI overlay |
| `packages/app/src/components/SimulationToolbar.tsx` | Play/pause/record controls |
| `packages/mcp/src/tools/simulation.ts` | MCP tools for training scripts |

### Data Structures

**Rust (vcad-kernel-physics):**

```rust
/// Simulation configuration
pub struct SimConfig {
    pub timestep: f64,           // seconds, default 0.001
    pub gravity: [f64; 3],       // m/s^2, default [0, 0, -9.81]
    pub iterations: u32,         // solver iterations, default 4
}

/// Observation returned each step
pub struct Observation {
    pub joint_positions: Vec<f64>,
    pub joint_velocities: Vec<f64>,
    pub joint_torques: Vec<f64>,
    pub contacts: Vec<Contact>,
    pub end_effector_pose: Option<Transform3D>,
}

/// Action to apply
pub enum Action {
    Torque(Vec<f64>),            // Direct torque control
    Position(Vec<f64>),          // Position targets (PD control)
    Velocity(Vec<f64>),          // Velocity targets
}

/// Collision shape derived from BRep
pub enum CollisionShape {
    ConvexHull(Vec<[f64; 3]>),
    Trimesh { vertices: Vec<[f64; 3]>, indices: Vec<[u32; 3]> },
    Box { half_extents: [f64; 3] },
    Cylinder { half_height: f64, radius: f64 },
    Sphere { radius: f64 },
}
```

**TypeScript (simulation-store):**

```typescript
interface SimulationState {
  status: 'idle' | 'running' | 'paused' | 'recording' | 'playback';
  simulationId: string | null;
  timestep: number;
  currentTime: number;
  recording: SimulationFrame[] | null;
  playbackIndex: number;
  visualizationOptions: {
    showContacts: boolean;
    showForces: boolean;
    showTrajectories: boolean;
    showCollisionShapes: boolean;
  };
}

interface SimulationFrame {
  time: number;
  jointStates: Map<string, JointState>;
  contacts: Contact[];
}
```

### Joint Mapping

| vcad Joint | phyz Joint |
|------------|--------------|
| `Revolute` | `JointType::Revolute` |
| `Prismatic` | `JointType::Prismatic` |
| `Fixed` | `JointType::Fixed` |
| `Ball` | `JointType::Spherical` |
| `Cylindrical` | Two stacked bodies: `JointType::Revolute` + `JointType::Prismatic` |

### BRep → Collision Shape Algorithm

1. For each part in document:
   a. If part is primitive (box/cylinder/sphere), use exact shape
   b. Else tessellate BRep to mesh
   c. Compute convex hull via quickhull
   d. If part is significantly non-convex (hull volume > 2x mesh volume):
      - Run VHACD decomposition → multiple convex shapes
   e. Store collision shape with part reference

### Dependencies

| Crate | Purpose |
|-------|---------|
| `phyz` | Articulated multibody physics engine |
| `phyz-collision` | GJK/EPA collision detection |
| `phyz-contact` | Contact resolution and friction |
| `vhacd` | Convex decomposition |
| `tang-la` | Linear algebra (shared with phyz) |

## Tasks

### Phase 1: Physics Engine Integration

- [ ] Create `vcad-kernel-physics` crate with phyz (`l`)
- [ ] Implement `SimConfig` and basic world setup (`s`)
- [ ] Map vcad joints to phyz joint types (`m`)
- [ ] Add `step()` and `reset()` methods (`s`)
- [ ] Implement gravity and basic dynamics (`xs`)

### Phase 2: Collision Shape Generation

- [ ] Implement primitive shape detection (box/cylinder/sphere) (`s`)
- [ ] Add convex hull generation from BRep tessellation (`m`)
- [ ] Integrate VHACD for concave shape decomposition (`m`)
- [ ] Add trimesh fallback for exact collision (`s`)
- [ ] Cache collision shapes per part hash (`s`)

### Phase 3: Gym Interface

- [ ] Define `Observation` and `Action` types (`xs`)
- [ ] Implement `step(action) → (obs, reward, done, info)` (`m`)
- [ ] Add position/velocity/torque control modes (`s`)
- [ ] Implement contact force observation (`s`)
- [ ] Add end-effector pose tracking (`s`)

### Phase 4: WASM Bindings

- [ ] Add WASM bindings in `vcad-kernel-wasm` (`m`)
- [ ] Create `packages/engine/src/physics.ts` wrapper (`s`)
- [ ] Expose simulation state to TypeScript (`s`)

### Phase 5: App Integration

- [ ] Create `simulation-store.ts` with state management (`s`)
- [ ] Build `SimulationMode.tsx` component (`m`)
- [ ] Add play/pause/record toolbar (`s`)
- [ ] Implement recording and playback (`m`)
- [ ] Add visualization overlays (contacts, forces, trajectories) (`m`)

### Phase 6: MCP Tools

- [ ] Add `create_simulation` tool (`s`)
- [ ] Add `step_simulation` tool (`s`)
- [ ] Add `reset_simulation` tool (`xs`)
- [ ] Add `set_action` tool (`xs`)
- [ ] Add `get_observation` tool (`xs`)
- [ ] Document MCP tools for training scripts (`s`)

## Acceptance Criteria

- [ ] Can create physics simulation from vcad document with joints
- [ ] phyz simulates rigid body dynamics with gravity
- [ ] Joint constraints map correctly (revolute, prismatic, fixed)
- [ ] Collision shapes auto-generated from BRep geometry
- [ ] Gym-style `step(action)` returns observations
- [ ] MCP tools enable external training scripts
- [ ] **Simulate Franka Panda arm** from URDF-imported document
- [ ] **Train simple reach policy** via MCP (arm reaches target position)
- [ ] App shows simulation mode with play/pause controls
- [ ] Contact points and forces visualized in viewport
- [ ] Simulation recording and playback works

## Future Enhancements

- [ ] GPU-accelerated physics via `phyz-gpu` batched WGPU kernels
- [ ] Domain randomization for sim-to-real transfer
- [ ] Parallel environment instances for faster training
- [ ] Soft body simulation (cloth, deformables)
- [ ] Fluid dynamics integration
- [ ] Inverse dynamics for trajectory optimization
- [ ] ROS 2 bridge for real robot deployment
- [ ] Isaac Gym-compatible observation/action spaces
- [ ] Reward function editor in app
- [ ] Pre-trained policy library for common tasks
