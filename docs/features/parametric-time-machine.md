# Parametric Time Machine

**Score: 68/100** | **Priority: #22**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |

> Verified against repo 2026-07-30. Vision doc — the continuous parameter↔physics loop, sweep recording, heatmaps, and Pareto optimizer described here are not implemented. Adjacent shipped pieces: property-panel scrub inputs (live parametric edits), `set_parameters`/`parameter_gradient`/`document_parameter_gradient` (analytic d(QoI)/d(parameter) via vcad-kernel-diff), and gym physics via MCP.

## Overview

Every design is a point in a continuous parameter space. Drag sliders and watch physics respond in real-time. FEEL the design space, don't just calculate it.

Traditional CAD treats parameters as discrete inputs requiring rebuild cycles. The Parametric Time Machine treats your design as a living system where every parameter adjustment instantly propagates through geometry and physics simulation simultaneously.

## Why It Matters

The traditional optimization workflow:
1. Change parameter
2. Rebuild geometry
3. Run simulation
4. Wait for results
5. Analyze
6. Repeat

Engineers develop intuition through iteration, but iteration is slow. Each cycle takes minutes to hours. Exploring 100 variations might take days.

**What if you could explore parameters as fluidly as scrubbing a video?**

When parameter exploration becomes instantaneous, you discover relationships you'd never find analytically:
- Non-linear interactions between parameters
- Critical thresholds where behavior changes dramatically
- Unexpected optima far from initial guesses
- Trade-off surfaces that inform design decisions

## Technical Implementation

### Core Architecture

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  Parameter UI   │────▶│  Geometry Engine │────▶│ Physics Engine  │
│   (sliders)     │     │  (parametric)    │     │ (continuous)    │
└─────────────────┘     └──────────────────┘     └─────────────────┘
         │                       │                        │
         └───────────────────────┴────────────────────────┘
                          Feedback Loop (<50ms)
```

### Parameter Binding

Parameters bind directly to geometry dimensions:
- Sketch dimensions (lengths, angles, radii)
- Feature parameters (extrude depth, fillet radius)
- Assembly constraints (offsets, joint limits)
- Material properties (density, stiffness)

### Continuous Physics

Physics simulation runs continuously as parameters change:
- Incremental solver updates (not full rebuilds)
- Warm-starting from previous state
- Adaptive time-stepping during rapid changes
- Interpolation for smooth visual transitions

### Recording and Analysis

- Record parameter sweeps as time-series data
- Export to CSV/Parquet for external analysis
- Replay sweeps at any speed
- Annotate interesting points in parameter space

### Multi-Parameter Exploration

- **1D sweeps**: Single slider, line plot of metrics
- **2D heatmaps**: Two parameters, colored grid showing response
- **Pareto frontiers**: Multi-objective optimization visualization
- **Sensitivity analysis**: Tornado charts showing parameter influence

## User Experience

```
┌─────────────────────────────────────────────────────────────────┐
│  Parameters                                                      │
├─────────────────────────────────────────────────────────────────┤
│  gripper_width: [━━━━━━●━━━━━] 40mm                             │
│  arm_length:    [━━━━━━━━●━━━] 300mm                            │
│  motor_torque:  [━━●━━━━━━━━━] 2.5 Nm                           │
│                                                                  │
│  [Record Sweep] [Optimize...] [Reset]                           │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│                                                                  │
│              [3D viewport with arm mechanism]                    │
│                                                                  │
│              Physics simulation running live                     │
│              Color overlay: stress distribution                  │
│                                                                  │
│              Max stress: 45 MPa                                  │
│              Deflection: 2.3mm                                   │
│              Clearance: OK                                       │
│                                                                  │
└─────────────────────────────────────────────────────────────────┘
```

### Interaction Modes

**Scrubbing**: Drag any slider, see immediate response. Physics interpolates smoothly.

**Linking**: Lock parameter ratios (e.g., width = 0.5 * length). Explore constrained subspaces.

**Pinning**: Pin metrics to viewport (stress, clearance, weight). Watch them update live.

**Recording**: Hit record, drag sliders, stop. Replay the exploration or export data.

## Advanced Features

### Automated Optimization

Define objectives and constraints in natural language:

```
"Optimize for maximum payload while keeping reach > 400mm"
```

The system:
1. Identifies relevant parameters
2. Runs thousands of physics simulations
3. Builds response surface model
4. Shows Pareto frontier of trade-offs
5. Click any point on frontier to see that design

### Optimization Interface

```
┌─────────────────────────────────────────────────────────────────┐
│  Optimization: Payload vs Weight                                 │
├─────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Payload    ●                                                    │
│    (kg)       ●                                                  │
│  5.0│          ●●                                                │
│     │            ●●●                                             │
│  2.5│               ●●●●                                         │
│     │                   ●●●●●                                    │
│  0.0└────────────────────────────                                │
│       0.5      1.0      1.5      2.0                             │
│              Weight (kg)                                         │
│                                                                  │
│  [Click any point to load that design]                          │
│  Selected: 3.2kg payload @ 1.1kg weight                         │
└─────────────────────────────────────────────────────────────────┘
```

### Sensitivity Heatmaps

Visualize how two parameters interact:

```
               arm_length (mm)
            200   250   300   350   400
          ┌─────┬─────┬─────┬─────┬─────┐
    30    │ ░░░ │ ░░░ │ ▒▒▒ │ ▓▓▓ │ ███ │
gripper   ├─────┼─────┼─────┼─────┼─────┤
_width 40 │ ░░░ │ ▒▒▒ │ ▓▓▓ │ ███ │ !!! │
 (mm)     ├─────┼─────┼─────┼─────┼─────┤
    50    │ ▒▒▒ │ ▓▓▓ │ ███ │ !!! │ !!! │
          └─────┴─────┴─────┴─────┴─────┘

░ Low stress  ▓ Medium stress  !!! Failure
```

## What Users Discover

Real insights from parameter exploration:

- **"There's a cliff at 35mm where it stops working"** — Threshold behaviors invisible in point samples become obvious when scrubbing.

- **"These two parameters interact — I didn't expect that"** — Heatmaps reveal coupling between seemingly independent dimensions.

- **"The optimum is nowhere near where I started"** — Initial intuition often wrong. Exploration finds unexpected sweet spots.

- **"I can trade 10% weight for 50% more stiffness"** — Pareto frontiers quantify trade-offs for informed decisions.

- **"The constraint I thought was binding isn't"** — Some limits have slack; others are tight. Instant feedback shows which.

## Success Metrics

| Metric | Target | Notes |
|--------|--------|-------|
| Parameter → physics response | <50ms | Perceptually instant |
| Frame rate during sweeps | 30fps | Smooth scrubbing |
| Geometry rebuild time | <20ms | Incremental updates |
| Optimization convergence | <60s | For typical 5-10 parameter problems |
| Pareto frontier resolution | 100+ points | Enough for smooth curve |

### Performance Requirements

- **Incremental geometry**: Only rebuild affected features, not entire model
- **Physics warm-start**: Use previous state as initial guess
- **LOD during motion**: Reduce mesh quality while scrubbing, refine when paused
- **Background optimization**: Run sweeps without blocking UI

## Competitive Advantage

This feature is **only possible** with vcad's unique architecture:

1. **Zero-latency parametric** — Geometry rebuilds in milliseconds, not seconds
2. **Always-on physics** — Simulation runs continuously, not on-demand
3. **Unified parameter space** — Geometry and physics share the same parameter definitions

Traditional CAD systems treat simulation as a separate post-process. By the time you get results, you've lost the exploratory momentum. The Parametric Time Machine keeps you in flow state, building intuition through direct manipulation.

**No other tool has both instant parametric updates AND continuous physics simulation.**

## Implementation Phases

### Phase 1: Parameter Sliders
- Bind sliders to geometry dimensions
- Instant geometry updates on drag
- Basic physics response

### Phase 2: Recording & Playback
- Record parameter sweeps
- Export time-series data
- Replay at variable speed

### Phase 3: Multi-Parameter Visualization
- 2D heatmaps
- Sensitivity analysis
- Parameter correlation views

### Phase 4: Automated Optimization
- Natural language objectives
- Pareto frontier generation
- Design space sampling

## Related Features

- **Always-On Physics** — Foundation for real-time response
- **Parametric DAG** — Enables incremental rebuilds
- **Assembly Constraints** — Parameters propagate through assemblies
- **AI Co-pilot** — Natural language parameter exploration
