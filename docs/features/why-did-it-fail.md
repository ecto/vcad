# "Why Did It Fail?" Button

**Score: 71/100** | **Priority: #19**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |

> Verified against repo 2026-07-30. State row added; no "Why?" button or AI failure-diagnosis pipeline found in the app. Adjacent shipped capability: simulation replay/trace tooling exists on the MCP side (`get_sim_replay`, `record_simulation`), but the diagnosis feature described here is not built.

## Overview

Simulation fails. Robot tips over. Gripper misses. One click: "Why?"

AI analyzes the simulation trace, identifies the root cause, and suggests actionable fixes. No more staring at plots or rewatching videos frame by frame.

## Why It Matters

- **Debugging physics is hard** — Failures manifest as symptoms (missed target, tipped robot) but root causes hide in the data (joint saturation at t=1.2s, contact force spike)
- **Root cause is often non-obvious** — Saturation, timing issues, geometry constraints, and cascading effects are difficult to trace manually
- **Engineers spend hours on what AI can diagnose in seconds** — Pattern matching across time-series data is exactly what LLMs excel at
- **Faster iteration = better designs** — Shorten the debug loop from hours to seconds, enabling more design exploration

## Technical Implementation

### Data Collection

Record full simulation trace during execution:

```typescript
interface SimulationTrace {
  timesteps: number[];
  joints: {
    positions: number[][];    // [timestep][joint]
    velocities: number[][];
    torques: number[][];
    limits: JointLimits[];
  };
  endEffector: {
    poses: Pose[];
    targetPose?: Pose;
  };
  contacts: ContactEvent[];
  stability: {
    centerOfMass: Vec3[];
    supportPolygon: Polygon[];
  };
}
```

### Anomaly Detection

AI analyzes trace for common failure patterns:

| Anomaly Type | Detection Method |
|--------------|------------------|
| **Saturation** | Joint velocity/torque at limit for >N timesteps |
| **Instability** | Oscillation frequency analysis, divergence detection |
| **Collision** | Unexpected contact events (not in planned path) |
| **Kinematic limits** | End effector target outside reachable workspace |
| **Balance loss** | CoM projection exits support polygon |

### Response Generation

1. Identify primary failure mode from trace analysis
2. Trace causality chain backwards (effect → cause)
3. Generate natural language explanation
4. Propose concrete, actionable fixes with one-click apply

## Example

User clicks "Why?" after gripper misses target:

```
AI Response:

The gripper missed because:

1. Joint 2 reached velocity limit (450 deg/s) at t=1.2s
2. This caused the joint to fall behind the planned trajectory
3. Position error accumulated to 43mm at the end effector
4. Additionally: target was 12mm outside the reachable workspace

Suggested fixes:
┌─────────────────────────────────────────────────────┐
│ ○ Reduce trajectory speed by 20%           [Apply] │
│ ○ Extend link 1 by 15mm to expand workspace [Apply] │
│ ○ Use higher-torque motor for joint 2    [Details] │
└─────────────────────────────────────────────────────┘
```

## Failure Categories

| Category | Symptoms | Detection | Typical Fixes |
|----------|----------|-----------|---------------|
| **Saturation** | Limit hit, error accumulates, sluggish response | Velocity/torque at bounds | Slow trajectory, bigger actuator, reduce payload |
| **Instability** | Oscillation, vibration, divergence | FFT analysis, growing amplitude | Tune gains, add damping, reduce stiffness |
| **Collision** | Unexpected contact, force spikes, deflection | Contact events outside plan | Modify path, change geometry, add clearance |
| **Workspace** | Target unreachable, joint limits hit | IK failure, position clipping | Extend links, reposition base, change target |
| **Balance** | Tipping, falling, recovery motions | CoM outside support polygon | Widen stance, slow down, counterweight |
| **Timing** | Late arrival, missed sync points | Trajectory completion time | Faster actuators, shorter path, predictive start |

## User Interface

### Trigger Points

- Automatic prompt when simulation detects failure condition
- Manual "Why?" button in simulation toolbar
- Right-click on any anomaly in trajectory plot

### Response Display

- Inline explanation in simulation panel
- Highlighted regions on trajectory plots showing anomalies
- 3D viewport annotations (workspace boundary, contact points)
- One-click fix buttons with preview

## Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| Root cause accuracy | >80% | User feedback on diagnosis correctness |
| Fix effectiveness | >60% | Applied fix resolves issue on retry |
| Analysis latency | <5 seconds | Time from click to response |
| User adoption | >70% | % of failures where button is clicked |

## Implementation Phases

### Phase 1: Core Analysis
- Trace recording infrastructure
- Basic anomaly detection (saturation, workspace)
- Natural language explanation generation

### Phase 2: Smart Fixes
- One-click fix application
- Fix preview before apply
- Multiple fix options with trade-offs

### Phase 3: Learning
- Learn from user feedback on diagnosis accuracy
- Improve fix suggestions based on what actually worked
- Project-specific pattern recognition

## Competitive Advantage

No simulation tool currently offers AI-powered failure diagnosis:

- **Traditional CAD** — Manual debugging, no AI assistance
- **Robot simulators** — Log files and plots, user interprets
- **Physics engines** — Raw data output, no analysis

vcad's "Why Did It Fail?" transforms simulation from a black box into a collaborative debugging partner.
