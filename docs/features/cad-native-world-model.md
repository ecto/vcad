# CAD-Native World Model

**Score: 75/100** | **Priority: #21** | **Category: The Endgame**

## Overview

Train world models on vcad's unified parametric CAD + physics simulation. Unlike video-based world models (Sora, Genie) or mesh-based simulators (Isaac), vcad provides **semantic structure**: joints, constraints, parameters, and ground-truth physics. The world model learns not just dynamics, but *how design affects dynamics*.

This is the long-term play: vcad becomes a **foundation model training platform for robotics**.

## Why It Matters

### The Data Problem in Robotics

| Approach | Data Source | Limitation |
|----------|-------------|------------|
| Real robots | Physical teleoperation | Expensive, slow, limited variation |
| Video prediction | YouTube, Ego4D | No physics grounding, no actions |
| Mesh simulation | Isaac, MuJoCo | Static geometry, no parametric variation |
| **vcad** | **Parametric CAD + Physics** | **Infinite variation, semantic structure, ground truth** |

### What Makes vcad Unique

**1. Parametric Domain Randomization**

Isaac randomizes textures. vcad randomizes *the robot itself*:

```python
for episode in range(1_000_000):
    # Vary the design, not just the lighting
    gripper_width = uniform(30, 60)  # mm
    arm_length = uniform(200, 400)   # mm
    motor_torque = uniform(1, 5)     # Nm

    robot = vcad.generate(template, {
        "gripper_width": gripper_width,
        "arm_length": arm_length,
        "motor_torque": motor_torque,
    })

    trajectory = vcad.simulate(robot, task="pick-place")
    dataset.append((robot.params, trajectory))
```

**2. Semantic State Representation**

```typescript
// Isaac gives you: [x, y, z, qw, qx, qy, qz] per body

// vcad gives you:
{
  // Semantic joint state
  joints: [
    { id: "shoulder", type: "revolute", angle: 45, velocity: 10, torque: 1.2 },
    { id: "elbow", type: "revolute", angle: 90, velocity: -5, torque: 0.8 },
    { id: "gripper", type: "slider", position: 20, velocity: 0, force: 5 },
  ],

  // The parameters that generated this robot
  geometry_params: {
    gripper_width: 40,
    arm_length: 300,
    link2_mass: 0.5,
  },

  // Semantic contact information
  contacts: [
    { bodyA: "gripper_left", bodyB: "object", point: [150, 0, 50], force: [0, 0, 9.8] },
  ],

  // Constraint satisfaction
  constraints: [
    { type: "joint_limit", joint: "elbow", satisfied: true, margin: 15 },
  ]
}
```

**3. Design-Conditioned Prediction**

Traditional world model: `s_{t+1} = f(s_t, a_t)`

vcad world model: `s_{t+1} = f(s_t, a_t, θ)` where `θ` = geometry parameters

This enables:
- Predict how design changes affect task success
- Gradient-based design optimization
- "What if" queries without re-simulation

## Technical Implementation

### Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     vcad World Model Stack                      │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌─────────────────┐                                            │
│  │ Robot Template  │  .vcad file with parametric dimensions     │
│  │ (arm.vcad)      │                                            │
│  └────────┬────────┘                                            │
│           │                                                     │
│           ▼                                                     │
│  ┌─────────────────┐                                            │
│  │   Parametric    │  Sample from parameter distributions       │
│  │   Generator     │  gripper_width ~ U(30, 60)                 │
│  └────────┬────────┘  arm_length ~ U(200, 400)                  │
│           │                                                     │
│           ▼                                                     │
│  ┌─────────────────┐                                            │
│  │ Physics Engine  │  phyz simulation                       │
│  │ (phyz/WASM)   │  Ground truth trajectories                 │
│  └────────┬────────┘                                            │
│           │                                                     │
│           ▼                                                     │
│  ┌─────────────────┐                                            │
│  │  Trajectory     │  (params, states, actions, outcomes)       │
│  │  Dataset        │  Parquet/HDF5 format                       │
│  └────────┬────────┘                                            │
│           │                                                     │
│           ▼                                                     │
│  ┌─────────────────┐                                            │
│  │  World Model    │  Transformer/SSM architecture              │
│  │  Training       │  Predicts next state + task success        │
│  └────────┬────────┘                                            │
│           │                                                     │
│           ▼                                                     │
│  ┌─────────────────┐                                            │
│  │  Applications   │  Planning, design optimization,            │
│  │                 │  policy distillation, sim-to-real          │
│  └─────────────────┘                                            │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

### Data Schema

```typescript
interface WorldModelSample {
  // Robot design (constant within episode)
  geometry_params: Record<string, number>;  // e.g., { gripper_width: 40, arm_length: 300 }

  // Timestep data
  timestep: number;
  dt: number;

  // State (semantic)
  joint_positions: number[];      // degrees or mm
  joint_velocities: number[];     // deg/s or mm/s
  joint_torques: number[];        // Nm or N
  end_effector_pose: number[];    // [x, y, z, qw, qx, qy, qz]

  // Action
  action_type: "torque" | "position" | "velocity";
  action_values: number[];

  // Contacts (variable length)
  contacts: {
    body_a: string;
    body_b: string;
    point: [number, number, number];
    normal: [number, number, number];
    force: number;
  }[];

  // Task outcome (for terminal states)
  task_success?: boolean;
  task_reward?: number;
}
```

### Model Architecture

```python
class VCADWorldModel(nn.Module):
    """
    Design-conditioned world model.

    Inputs:
        - geometry_params: [batch, num_params] - robot design parameters
        - state_history: [batch, seq_len, state_dim] - past states
        - action: [batch, action_dim] - action to take

    Outputs:
        - next_state: [batch, state_dim] - predicted next state
        - success_prob: [batch, 1] - task success probability
        - param_gradient: [batch, num_params] - d(success)/d(params)
    """

    def __init__(self, config):
        # Geometry encoder - learns design → dynamics mapping
        self.geometry_encoder = MLP(
            input_dim=config.num_params,
            hidden_dim=256,
            output_dim=config.geometry_embedding_dim,
        )

        # State encoder - handles variable contacts
        self.state_encoder = TransformerEncoder(
            d_model=config.state_embedding_dim,
            nhead=8,
            num_layers=4,
        )

        # Dynamics predictor - conditioned on geometry
        self.dynamics = FiLM_MLP(
            state_dim=config.state_embedding_dim,
            condition_dim=config.geometry_embedding_dim,
            output_dim=config.state_dim,
        )

        # Success predictor
        self.success_head = MLP(
            input_dim=config.state_embedding_dim + config.geometry_embedding_dim,
            output_dim=1,
        )

    def forward(self, geometry_params, state_history, action):
        # Encode geometry
        geom_embed = self.geometry_encoder(geometry_params)

        # Encode state history
        state_embed = self.state_encoder(state_history)

        # Predict next state (conditioned on geometry)
        next_state = self.dynamics(state_embed, action, condition=geom_embed)

        # Predict success probability
        success_prob = torch.sigmoid(self.success_head(
            torch.cat([state_embed, geom_embed], dim=-1)
        ))

        return next_state, success_prob
```

### Training Pipeline

**CLI Interface:**

```bash
# 1. Generate dataset from parametric template
vcad generate-dataset \
  --template robots/arm-3dof.vcad \
  --param-ranges "gripper_width:30-60,arm_length:200-400,motor_torque:1-5" \
  --task pick-place \
  --episodes 1000000 \
  --parallel 8 \
  --output dataset/arm-pick-place.parquet

# 2. Train world model
vcad train-world-model \
  --dataset dataset/arm-pick-place.parquet \
  --architecture transformer \
  --epochs 100 \
  --batch-size 256 \
  --output models/arm-world-model.onnx

# 3. Evaluate on held-out designs
vcad eval-world-model \
  --model models/arm-world-model.onnx \
  --test-set dataset/arm-pick-place-test.parquet \
  --metrics "mse,success_auc,design_gradient_correlation"

# 4. Use for planning
vcad plan \
  --robot robots/my-arm.vcad \
  --world-model models/arm-world-model.onnx \
  --task "move object from [100,0,50] to [300,200,100]" \
  --method mpc
```

**Browser Interface:**

```
┌─────────────────────────────────────────────────────────────────┐
│  World Model Training                                           │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  Template: [arm-3dof.vcad     ▼]    Task: [pick-place  ▼]      │
│                                                                 │
│  Parameter Ranges:                                              │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │ gripper_width   [30 ]mm ────────────────── [60 ]mm     │   │
│  │ arm_length      [200]mm ────────────────── [400]mm     │   │
│  │ motor_torque    [1  ]Nm ────────────────── [5  ]Nm     │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
│  Episodes: [1,000,000]     Parallel workers: [8]                │
│                                                                 │
│  [Generate Dataset]  [Train Model]  [Export ONNX]               │
│                                                                 │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  Training Progress                                       │   │
│  │  ████████████████████░░░░░░░░░░  67%                    │   │
│  │                                                          │   │
│  │  Loss: 0.0023  Success AUC: 0.94  Epoch: 67/100         │   │
│  └─────────────────────────────────────────────────────────┘   │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

## What the World Model Learns

| Level | Capability | Example Query |
|-------|------------|---------------|
| **Dynamics** | Physics prediction | "Where will the arm be in 0.5s?" |
| **Kinematics** | Workspace reasoning | "Can this config reach [300, 200, 100]?" |
| **Contact** | Grasp prediction | "Will this grasp succeed?" |
| **Design→Dynamics** | Parameter effects | "How does gripper_width affect grasp success?" |
| **Optimization** | Design gradients | "Which parameter should I change to improve success?" |

### Novel Capabilities

**1. Design Optimization via Backprop**

```python
# Find optimal gripper width for a task
gripper_width = torch.tensor([40.0], requires_grad=True)
optimizer = torch.optim.Adam([gripper_width], lr=1.0)

for step in range(100):
    # Predict success with current design
    params = {"gripper_width": gripper_width, "arm_length": 300, ...}
    _, success_prob = world_model(params, initial_state, planned_actions)

    # Maximize success probability
    loss = -success_prob
    loss.backward()
    optimizer.step()

print(f"Optimal gripper width: {gripper_width.item():.1f}mm")
```

**2. "What If" Design Queries**

```
User: "Would a longer arm help with this task?"

AI: Running counterfactual simulation...

    Current design (arm_length=300mm): 73% success rate

    Counterfactuals:
    - arm_length=250mm: 61% success (-12%)
    - arm_length=350mm: 82% success (+9%)
    - arm_length=400mm: 79% success (+6%, diminishing returns)

    Recommendation: Extend arm to 350mm for optimal reach/torque tradeoff.
```

**3. Sim-to-Real with Design Transfer**

Train on parametric variation → deploy on specific real robot:

```python
# Training: vary everything
train_params = {
    "gripper_width": Uniform(30, 60),
    "arm_length": Uniform(200, 400),
    "friction": Uniform(0.3, 0.8),
    "motor_noise": Uniform(0, 0.1),
}

# Deployment: condition on real robot's measured params
real_robot_params = {
    "gripper_width": 42.3,  # Measured
    "arm_length": 312.5,    # Measured
    "friction": 0.55,       # Estimated
    "motor_noise": 0.05,    # Calibrated
}

# World model generalizes to real robot
policy = world_model.plan(real_robot_params, task)
```

## Applications

### 1. Model-Based RL

Use world model for planning instead of model-free RL:

```python
def mpc_policy(state, world_model, horizon=20):
    best_actions = None
    best_reward = -inf

    for _ in range(1000):  # CEM iterations
        # Sample action sequences
        actions = sample_actions(horizon)

        # Rollout in world model (fast, differentiable)
        predicted_states, rewards = world_model.rollout(state, actions)

        if rewards.sum() > best_reward:
            best_reward = rewards.sum()
            best_actions = actions

    return best_actions[0]  # Execute first action
```

### 2. Design-Aware Policy

Policy that adapts to robot geometry:

```python
class DesignAwarePolicy(nn.Module):
    def __init__(self, world_model):
        self.world_model = world_model  # Frozen
        self.policy_head = MLP(...)

    def forward(self, state, geometry_params):
        # Get geometry embedding from world model
        geom_embed = self.world_model.geometry_encoder(geometry_params)

        # Policy conditioned on design
        action = self.policy_head(state, geom_embed)
        return action
```

### 3. Automated Design Optimization

```bash
vcad optimize-design \
  --template arm.vcad \
  --world-model arm-world-model.onnx \
  --objective "maximize grasp_success while mass < 2kg" \
  --constraints "arm_length >= 200mm, gripper_width >= 30mm" \
  --method bayesian \
  --iterations 1000 \
  --output optimized-arm.vcad
```

### 4. Foundation Model Pre-training

Train on diverse vcad robots, fine-tune on specific tasks:

```
Pre-training data:
- 1000 robot templates (arms, grippers, mobile bases, humanoids)
- 100 parameter variations each
- 10 tasks each
- = 1B state transitions

Fine-tuning:
- Your specific robot
- Your specific task
- 1000 real-world demonstrations
- = Efficient transfer
```

## Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| **Prediction accuracy** | <5mm position error at t+1s | Held-out test set |
| **Success prediction AUC** | >0.90 | Binary classification |
| **Design gradient correlation** | >0.8 | Compare to finite-difference |
| **Planning success rate** | >80% on novel designs | Zero-shot generalization |
| **Sim-to-real transfer** | >70% real success from sim-trained | Physical robot eval |
| **Design optimization lift** | >20% improvement over baseline | A/B test |

## Competitive Advantage

| Aspect | NVIDIA Isaac | Google RT-X | vcad |
|--------|--------------|-------------|------|
| Data source | Static meshes | Real robots | **Parametric CAD** |
| Variation | Texture/lighting | Limited robots | **Infinite geometry** |
| Semantics | Collision shapes | Images | **Joints, constraints, params** |
| Differentiable | No | No | **Yes (design gradients)** |
| Cost to scale | $$$$ (GPUs) | $$$$$ (robots) | **$ (browser tabs)** |
| Open | No | Partially | **Yes** |

## Implementation Phases

### Phase 1: Data Generation (2-3 weeks)
- [ ] Dataset schema and Parquet writer
- [ ] Parametric sampling from .vcad templates
- [ ] Parallel episode generation (Web Workers / Rayon)
- [ ] CLI: `vcad generate-dataset`

### Phase 2: Model Training (3-4 weeks)
- [ ] PyTorch dataset loader
- [ ] Geometry-conditioned transformer architecture
- [ ] Training loop with logging (W&B)
- [ ] CLI: `vcad train-world-model`

### Phase 3: Inference & Planning (2-3 weeks)
- [ ] ONNX export for deployment
- [ ] MPC planner using world model
- [ ] Design optimization loop
- [ ] CLI: `vcad plan`, `vcad optimize-design`

### Phase 4: Browser Integration (2-3 weeks)
- [ ] ONNX Runtime Web for in-browser inference
- [ ] Training UI (if feasible, or offload to server)
- [ ] Design optimization UI
- [ ] "What if" query interface

## Dependencies

| Dependency | Purpose | Status |
|------------|---------|--------|
| WASM Physics | Data generation in browser | P0 — needed first |
| Gym interface | Episode collection | Exists (vcad-kernel-physics) |
| PyTorch | Model training | External |
| ONNX Runtime | Inference | Available for Rust/WASM |

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Model doesn't generalize across designs | Start with simple parametric variation, increase complexity |
| Sim-to-real gap | Include noise/friction variation in training |
| Compute requirements | Start with small models, distill to efficient architectures |
| Data generation bottleneck | Parallelize across browser tabs / cloud workers |

## Related Features

- [WASM Physics Integration](./wasm-physics-integration.md) — Required foundation
- [One-Click RL Training](./one-click-rl-training.md) — Uses world model for planning
- [Parametric Time Machine](./parametric-time-machine.md) — UI for exploring param space
- [AI Co-Designer](./ai-co-designer.md) — Uses world model for "what if" queries

## The Vision

> vcad becomes not just a CAD tool, but a **foundation model training platform for robotics**. Every parametric robot designed in vcad contributes to a shared world model that understands how design affects dynamics. This is the data flywheel that no competitor can replicate.

```
Design robots in vcad
        ↓
    Train world models
        ↓
    Better policies
        ↓
    More users design in vcad
        ↓
    Better world models
        ↓
    [flywheel continues]
```
