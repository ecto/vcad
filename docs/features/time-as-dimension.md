> **Note (2026-07-30):** Much of this vision shipped in a diverged form as the document animation timeline (`doc.timeline`): `AnimationTimeline.tsx` + `useTimelinePlayback` in the app, and the `animate`, `timeline_from_simulation`, `render_sequence`, and `export_video` MCP tools. Ghost rendering, trajectory trails, and branching timelines remain aspirational.

# Time as a Dimension

**Score: 77/100** | **Priority: #13**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |
| Owner | `unassigned` |

> Verified against repo 2026-07-30.

## Overview

Scrub through simulation time like scrubbing a video. See the mechanism at t=0, t=0.5s, t=2s. Keyframe joint positions. View trajectories as ghosted trails.

Traditional CAD treats geometry as frozen in space. vcad treats time as a first-class dimension—mechanisms don't just exist, they *move*.

## Why It Matters

**Mechanisms exist in time, not just space.** A robotic arm isn't a static collection of links—it's a dance of coordinated motion. Understanding that motion requires seeing the full trajectory, not just snapshots.

**Traditional CAD separates modeling from simulation.** You design in one tool, export to another for motion analysis, then struggle to correlate results back to your geometry. The feedback loop is broken.

**Debugging motion requires temporal context.** When something goes wrong at t=1.2s, you need to ask: "What was the gripper position? What forces were acting? What happened 100ms before?" Without time as a navigable dimension, these questions require re-running simulations and guessing.

**Design intuition comes from seeing motion.** Engineers develop better mechanisms when they can *feel* the motion—scrub back and forth, slow down the critical moment, see how parts relate across time.

## Technical Implementation

### State Recording

Physics state recorded at configurable intervals during simulation:

```typescript
interface TimelineState {
  timestamp: number;           // seconds
  jointPositions: Float32Array; // all joint values
  jointVelocities: Float32Array;
  instanceTransforms: Mat4[];   // world transforms per instance
}
```

### Efficient Storage

Delta compression minimizes memory for long simulations:

- **Keyframes**: Full state every N frames (e.g., every 100)
- **Deltas**: Only changed values between keyframes
- **Quantization**: Joint angles stored as 16-bit fixed-point when precision allows

Target: 1000 timesteps in <10MB for typical assemblies.

### Scrubber Seek

When user drags to time `t`:

1. Find nearest keyframe before `t`
2. Apply deltas forward to reach `t`
3. Update all instance transforms
4. Re-render viewport

For <50ms latency, pre-compute transforms at regular intervals and interpolate.

### Ghost Rendering

Show part at multiple timesteps simultaneously:

```typescript
interface GhostConfig {
  count: number;        // how many ghosts (e.g., 5)
  spacing: number;      // time between ghosts (e.g., 0.1s)
  direction: 'past' | 'future' | 'both';
  opacity: number;      // base opacity (fades with distance)
}
```

Render as semi-transparent instances with decreasing opacity. Use instanced rendering—ghosts share geometry, only transforms differ.

### Trajectory Trails

Track specific points over time:

```typescript
interface TrajectoryPoint {
  instanceId: string;
  localPosition: Vec3;  // point on the part to track
}
```

Record world position at each timestep. Render as line strip or tube geometry. Color-code by velocity or time.

## User Experience

### Timeline Bar

Horizontal bar at bottom of viewport:

```
|--[===]------------------●-----●---------●---------|
 0s      current        keyframe        keyframe   10s
         position
```

- **Scrubber handle**: Drag to seek
- **Keyframe markers**: Click to jump, double-click to edit
- **Time labels**: Show current time, total duration
- **Zoom**: Scroll to zoom timeline, drag to pan

### Interactions

| Action | Result |
|--------|--------|
| Drag scrubber | Mechanism animates to that state |
| Click timeline | Jump to that time |
| Shift+click | Add keyframe at current joint positions |
| Right-click keyframe | Edit, delete, or copy keyframe |
| Spacebar | Play/pause from current position |
| Mouse wheel on timeline | Zoom in/out |

### Ghost Toggle

Toolbar button or keyboard shortcut (G) toggles ghost mode:

- Shows 5 semi-transparent copies at regular intervals
- Past ghosts fade toward blue, future toward orange
- Adjust count and spacing in settings

### Trajectory Visualization

Right-click any point on a part → "Show trajectory":

- Draws path line from t=0 to t=end
- Color gradient shows time progression
- Hover on trail shows timestamp
- Multiple trajectories can be active simultaneously

Example: "Show trajectory of gripper tip" draws the path the end effector follows through space.

## Features

| Feature | Description |
|---------|-------------|
| **Scrub** | Drag to any time, mechanism state updates instantly |
| **Keyframes** | Define joint positions at specific times for animation |
| **Ghosts** | Semi-transparent past/future states rendered simultaneously |
| **Trails** | Path lines showing trajectory of tracked points |
| **Branch** | "What if it went left?" Create alternate timeline branches |
| **Slow-mo** | Playback at 0.1x, 0.25x, 0.5x speed |
| **Loop** | Loop playback between two time points |
| **Export** | Export animation as video or image sequence |

### Branching Timelines

For "what if" exploration:

1. Pause at decision point (e.g., t=1.0s)
2. Create branch
3. Modify joint command or constraint
4. Simulate forward
5. Compare branches side-by-side or overlaid

Branches stored as separate state sequences sharing common prefix.

## Success Metrics

| Metric | Target | Rationale |
|--------|--------|-----------|
| Scrub latency | <50ms | Feels instant, enables fluid exploration |
| Timesteps storable | 1000+ | 10 seconds at 100Hz, typical simulation |
| Ghost render overhead | <5ms | Negligible impact on frame rate |
| Memory per timestep | <10KB | 1000 steps = 10MB, reasonable |
| Keyframe seek | <20ms | Instant jump to any keyframe |

## Competitive Advantage

**No CAD tool treats time as a first-class dimension.**

- SolidWorks Motion: Separate study, results in charts
- Fusion 360: Animation timeline exists but disconnected from simulation
- Onshape: No native motion simulation
- FreeCAD: Animation workbench is primitive

vcad's approach: Time is as navigable as X, Y, Z. Scrub through a mechanism's life like scrubbing through a video. See ghosts of where it was and where it's going. Debug motion by *being there* at any moment.

This fundamentally changes how engineers understand mechanisms—from analyzing data about motion to *experiencing* motion directly.

## Future Extensions

- **Time-based constraints**: "Gripper must be closed by t=2s"
- **Collision timeline**: Scrub to see all collision events
- **Force visualization over time**: See how loads change
- **Multi-body comparison**: Overlay two different designs' motions
- **VR timeline**: Physically grab the scrubber in VR
