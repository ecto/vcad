# One-Click RL Training

**Score: 67/100** | **Priority: #23**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. Foundation shipped: gym-style physics environments via phyz (`crates/vcad-kernel-physics`) with MCP tools `create_robot_env`, `gym_step`/`gym_reset`/`gym_observe`/`gym_close`, plus batched `batch_create_envs`/`batch_step`/`batch_reset` — torque/position/velocity action modes as specced. Not shipped: `vcad train` CLI, in-repo PPO/SAC/TD3 implementations, browser training panel, ONNX policy export. (`packages/training` is an ML data-generation pipeline, not RL training.)

## Overview

```bash
vcad train --env pick-place --algo PPO
```

That's it. No Isaac setup, no Docker, no Python environment hell. Design a robot, train a policy, export to deploy.

vcad integrates reinforcement learning directly into the CAD workflow. The same tool you use to design your robot is the same tool you use to train its brain.

## Why It Matters

### The Current State of Robot RL

Setting up Isaac Gym today:
1. Install NVIDIA drivers (hope they're compatible)
2. Install Docker and nvidia-container-toolkit
3. Download Omniverse Launcher
4. Install Isaac Sim (50+ GB)
5. Configure conda environments
6. Debug CUDA version mismatches
7. Find the right IsaacGymEnvs commit that works
8. Pray

**Time to first training run: 2-5 days**

Most roboticists just want to train a policy. They don't want to become DevOps experts.

### The vcad Approach

```bash
vcad train robot.vcad --algo PPO --steps 1M
```

**Time to first training run: < 5 minutes**

- Design the robot in the same tool where you train it
- No external dependencies beyond vcad itself
- Works on any machine — Mac, Windows, Linux, browser
- Democratizes robot learning for students, hobbyists, and researchers

## Technical Implementation

### Gym-Compatible Environment

vcad documents automatically expose a gym-style interface:

```typescript
interface VcadEnv {
  reset(): Observation;
  step(action: Action): [Observation, number, boolean, Info];
  render(): void;
}
```

The physics simulation (built on the same kernel as the CAD system) provides:
- Rigid body dynamics with contact
- Joint constraints (revolute, prismatic, fixed)
- Configurable timestep and substeps

### Observation Space

| Component | Shape | Description |
|-----------|-------|-------------|
| Joint positions | `(n_joints,)` | Current joint angles/positions |
| Joint velocities | `(n_joints,)` | Current joint velocities |
| End effector pose | `(7,)` | Position (3) + quaternion (4) |
| Target pose | `(7,)` | Goal position + orientation |
| Object states | `(n_objects, 13)` | Pose + velocity per object |

### Action Space

Three modes, configurable per environment:

| Mode | Units | Use Case |
|------|-------|----------|
| Torque | Nm | Direct motor control |
| Position | degrees/mm | Position servos |
| Velocity | deg/s, mm/s | Velocity control |

### Built-in RL Algorithms

Implemented in pure TypeScript/Rust — no Python required:

- **PPO** (Proximal Policy Optimization) — default, stable, sample-efficient
- **SAC** (Soft Actor-Critic) — better for continuous control
- **TD3** (Twin Delayed DDPG) — robust to hyperparameters

All algorithms support:
- Parallel environment rollouts
- Automatic normalization
- Configurable network architectures
- Checkpointing and resumption

### Parallelization

**Browser:** Web Workers for parallel environments (8-32 typical)

**Native:** Thread pool with work-stealing (scales to CPU cores)

**GPU (future):** WebGPU compute shaders for batched simulation

## CLI Interface

### Training

```bash
# Basic training
vcad train robot.vcad --algo PPO --steps 1M --output policy.onnx

# With configuration
vcad train robot.vcad \
  --algo SAC \
  --steps 5M \
  --lr 3e-4 \
  --batch-size 256 \
  --n-envs 16 \
  --output policy.onnx

# Resume from checkpoint
vcad train robot.vcad --resume checkpoint.vcad --steps 2M

# With live visualization
vcad train robot.vcad --algo PPO --render
```

### Evaluation

```bash
# Evaluate policy performance
vcad eval robot.vcad --policy policy.onnx --episodes 100

# Output:
# Episodes: 100
# Mean reward: 245.3 ± 12.1
# Success rate: 94%
# Mean episode length: 127.4 steps
```

### Policy Export

```bash
# Export to ONNX for deployment
vcad export-policy policy.onnx --format onnx

# Export to TensorFlow Lite for edge devices
vcad export-policy policy.onnx --format tflite

# Export to C header for embedded
vcad export-policy policy.onnx --format c-array
```

## Browser Interface

Training is fully accessible from the web UI:

1. **Open robot document** — your assembled robot with joints defined
2. **Click "Train" in toolbar** — opens training panel
3. **Select task type**:
   - Reach (move end effector to target)
   - Pick-Place (grasp and move object)
   - Track (follow trajectory)
   - Custom (define reward function)
4. **Configure hyperparameters** — or use smart defaults
5. **Click "Start Training"**

### Real-time Visualization

- **Training curve** — reward over time, updating live
- **Policy rollout** — watch the robot execute current policy in viewport
- **Metrics dashboard** — success rate, episode length, loss curves
- **Environment preview** — see parallel envs as thumbnails

### Export

- Download trained policy as ONNX
- Save training run to document for reproducibility
- Share policy via link

## Pre-built Tasks

| Task | Description | Observation | Reward |
|------|-------------|-------------|--------|
| **Reach** | Move end effector to target position | joint state + target pos | `-distance_to_target` |
| **Pick-Place** | Grasp object, move to goal location | joint state + object pose + goal | `+1` on success, shaped distance reward |
| **Track** | Follow reference trajectory | joint state + trajectory point | `-tracking_error` |
| **Balance** | Keep robot upright (bipeds, etc.) | joint state + base orientation | `+1` per timestep upright |
| **Push** | Push object to target | joint state + object pose + target | `-object_distance_to_target` |
| **Insert** | Peg-in-hole insertion | joint state + peg/hole poses | `+1` on insertion, `-force` penalty |

### Custom Rewards

Define custom reward functions in the document:

```json
{
  "reward": {
    "type": "custom",
    "terms": [
      { "weight": 1.0, "type": "distance", "from": "end_effector", "to": "target" },
      { "weight": 0.1, "type": "energy", "sign": -1 },
      { "weight": 10.0, "type": "success", "condition": "distance < 0.01" }
    ]
  }
}
```

## Success Metrics

| Metric | Target |
|--------|--------|
| Time from robot design to training start | < 5 minutes |
| Training throughput (steps/sec, 8 envs) | > 10,000 |
| Reach task solve time | < 10 minutes |
| Pick-place task solve time | < 1 hour |
| Benchmark parity with Isaac Gym | Within 2x on standard tasks |
| Policy export formats | ONNX, TFLite, C array |

## Competitive Advantage

| Aspect | Isaac Gym | vcad |
|--------|-----------|------|
| Setup time | 2-5 days | 5 minutes |
| Dependencies | NVIDIA GPU, Docker, Omniverse | None (runs in browser) |
| Design-to-train workflow | Export URDF → import → configure | Same tool |
| Platform support | Linux only | Mac, Windows, Linux, Web |
| GPU required | Yes | No (optional acceleration) |
| Cost | Free (NVIDIA hardware required) | Free |

**Isaac takes days to set up. vcad takes one command.**

## Implementation Phases

### Phase 1: Core Training Loop
- Gym-compatible environment wrapper
- PPO implementation
- CLI `vcad train` command
- ONNX export

### Phase 2: Browser Integration
- Training panel UI
- Real-time visualization
- Pre-built task library

### Phase 3: Advanced Features
- SAC and TD3 algorithms
- Curriculum learning
- Domain randomization
- Sim-to-real tools

### Phase 4: Performance
- WebGPU acceleration
- Distributed training
- Cloud training integration

## Related Features

- [Physics Simulation](/docs/features/physics-simulation.md) — underlying dynamics engine
- [Assembly Mode](/docs/features/assembly-mode.md) — robot joint definitions
- [MCP Server](/docs/features/mcp-server.md) — programmatic training API
