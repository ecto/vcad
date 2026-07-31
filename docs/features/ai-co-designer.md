# AI Co-Designer

Intelligent design partner that understands geometry, physics, and engineering tradeoffs.

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |
| Owner | `unassigned` |
| Priority | `p1` |
| Score | 78/100 |
| Effort | `xl` (6-8 weeks) |
| Depends On | MCP Server, Physics Simulation |

> Verified against repo 2026-07-30. State corrected `proposed` → `in-progress`: much of the analysis surface has shipped as MCP tools under different names (`measure`, `check_clearance`, `inspect_cad` mass properties, `describe_scene`, `gym_*` simulation, `critique_route`), though the in-app AI chat panel and preview-overlay UI described below are not built.

## Problem

Current AI CAD tools are command executors, not design partners:

| User Says | Current AI | Real Engineering Need |
|-----------|------------|----------------------|
| "Make a box" | Makes a box | Already solved |
| "This arm needs to reach that shelf" | "I don't understand" | Requires geometry analysis, kinematics, tradeoff evaluation |
| "Why does my gripper miss the target?" | Can't help | Needs simulation, root cause analysis, actionable fixes |

**The gap:** AI can execute instructions but can't think alongside engineers. Real design involves:

1. **Spatial reasoning** — "Can this arm reach that point? What's blocking it?"
2. **Physics understanding** — "Will this motor stall under load? Where's the weak point?"
3. **Tradeoff analysis** — "If I extend this link, what else changes?"
4. **Iterative refinement** — "The last change broke clearance — fix it"

Engineers spend hours on analysis that AI could do in seconds — if it could see the design.

## Solution

AI as co-designer with full context: geometry, joints, constraints, physics state.

```
┌─────────────────────────────────────────────────────────────────┐
│                         vcad Document                           │
│  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────────────┐   │
│  │ Geometry│  │ Joints  │  │Constraints│ │ Physics State  │   │
│  │ (BRep)  │  │ (FK/IK) │  │ (Solver) │  │ (phyz)       │   │
│  └────┬────┘  └────┬────┘  └────┬────┘  └───────┬─────────┘   │
│       └───────────┴───────────┴─────────────────┘              │
│                           │                                     │
│                    MCP Context                                  │
└───────────────────────────┼─────────────────────────────────────┘
                            ▼
                    ┌───────────────┐
                    │  AI Co-Designer│
                    │  (Claude/GPT)  │
                    └───────┬───────┘
                            │
            ┌───────────────┼───────────────┐
            ▼               ▼               ▼
       ┌─────────┐    ┌─────────┐    ┌─────────┐
       │ Analyze │    │ Suggest │    │ Execute │
       │ Design  │    │ Options │    │ Changes │
       └─────────┘    └─────────┘    └─────────┘
```

### Core Capabilities

**1. Design Analysis**
- Measure distances, angles, clearances between any entities
- Compute workspace reachability for articulated mechanisms
- Identify interference and collision risks
- Calculate mass properties, center of gravity, moments of inertia

**2. Physics Reasoning**
- Run simulations and interpret results
- Identify failure modes (joint limits, motor saturation, instability)
- Predict behavior under load
- Compare dynamic performance of variants

**3. Tradeoff Evaluation**
- Quantify impact of parameter changes
- Present options with pros/cons
- Rank solutions by user-specified criteria
- Explain why one approach is better than another

**4. Actionable Suggestions**
- Propose specific parameter changes with previews
- One-click application of suggestions
- Maintain design intent through changes
- Undo-friendly atomic operations

### Conversation Examples

**Reach Analysis:**
```
User: "This arm needs to reach that shelf"

AI: Looking at the geometry...

    Current state:
    - Arm tip at (340, 0, 200) mm
    - Shelf at (520, 0, 180) mm
    - Gap: 180mm horizontally

    The arm can't reach. Options:

    1. Extend Link 2 by 180mm
       + Simple change
       - Torque at Joint 1 increases 40%
       - May need stronger motor

    2. Add a 4th revolute joint
       + Maintains torque margins
       - Adds 200g mass
       - More complex control

    3. Move base 180mm toward shelf
       + No arm changes
       - Requires base redesign

    [Preview Option 1] [Preview Option 2] [Preview Option 3]
```

**Failure Diagnosis:**
```
User: "Why does the gripper keep missing?"

AI: I ran 10 grasp simulations. Here's what's happening:

    Root cause: Joint 2 hits velocity limit at t=1.2s

    Timeline:
    - t=0.0s: Trajectory starts, all joints nominal
    - t=1.2s: Joint 2 saturates at 180°/s (limit)
    - t=1.4s: Position error accumulates to 43mm
    - t=1.6s: Gripper arrives 43mm left of target

    Fixes:
    1. Reduce trajectory speed 20% → stays within limits
    2. Upgrade Joint 2 motor (need 220°/s for current trajectory)

    [Apply Fix 1] [Apply Fix 2] [Show Simulation]
```

**Design Optimization:**
```
User: "Make this bracket lighter but keep it strong enough"

AI: Analyzing current bracket...

    Current: 847g, safety factor 3.2 (von Mises @ 120 MPa)

    I found 3 optimization opportunities:

    1. Reduce wall thickness 8mm → 5mm
       Mass: 847g → 612g (-28%)
       Safety factor: 3.2 → 2.1 (still safe)

    2. Add topology-optimized cutouts
       Mass: 847g → 534g (-37%)
       Safety factor: 3.2 → 1.8 (marginal)

    3. Switch to aluminum 7075-T6
       Mass: 847g → 298g (-65%)
       Safety factor: 3.2 → 2.4 (stronger material)

    Recommendation: Option 3 if budget allows, else Option 1.

    [Apply Option 1] [Apply Option 3] [Show FEA Results]
```

**Not included:** Autonomous design (AI proposes designs unprompted), manufacturing process planning, cost estimation.

## MCP Tools

### Read-Only Tools

| Tool | Description |
|------|-------------|
| `measure_distance` | Distance between two entities (point, edge, face) |
| `measure_angle` | Angle between edges, faces, or axes |
| `check_clearance` | Minimum distance between two parts |
| `get_workspace` | Reachable volume for articulated mechanism |
| `get_mass_properties` | Mass, CoG, inertia tensor for part/assembly |
| `analyze_constraints` | Constraint status, DOF, over/under-constrained |
| `get_joint_limits` | Position, velocity, torque limits per joint |

### Simulation Tools

| Tool | Description |
|------|-------------|
| `simulate_motion` | Run forward dynamics, return trajectory |
| `simulate_grasp` | Test grasp success with contact analysis |
| `check_interference` | Detect collisions through motion range |
| `compute_torques` | Inverse dynamics for given trajectory |
| `find_singularities` | Identify kinematic singularities |

### Modification Tools

| Tool | Description |
|------|-------------|
| `modify_dimension` | Change a parameter with preview |
| `add_joint` | Insert joint between parts |
| `apply_suggestion` | Execute a previously generated suggestion |
| `create_variant` | Clone current design for comparison |

### Context Tools

| Tool | Description |
|------|-------------|
| `get_document_context` | Full document state for conversation |
| `get_selection` | Currently selected entities |
| `get_history` | Recent operations for context |

## UX Details

### Chat Panel

```
┌─────────────────────────────────────────────────────────────┐
│ AI Co-Designer                                    [−] [×]   │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ User: This arm needs to reach that shelf               │ │
│ └─────────────────────────────────────────────────────────┘ │
│                                                             │
│ ┌─────────────────────────────────────────────────────────┐ │
│ │ AI: Looking at the geometry...                         │ │
│ │                                                         │ │
│ │ Current arm length: 340mm                              │ │
│ │ Shelf distance: 520mm                                  │ │
│ │ Gap: 180mm                                             │ │
│ │                                                         │ │
│ │ Options:                                               │ │
│ │ ┌─────────────┐ ┌─────────────┐ ┌─────────────┐       │ │
│ │ │ 1. Extend   │ │ 2. Add DOF  │ │ 3. Move base│       │ │
│ │ │    Link 2   │ │             │ │             │       │ │
│ │ │  [Preview]  │ │  [Preview]  │ │  [Preview]  │       │ │
│ │ └─────────────┘ └─────────────┘ └─────────────┘       │ │
│ └─────────────────────────────────────────────────────────┘ │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│ [Ask about this design...]                           [Send] │
└─────────────────────────────────────────────────────────────┘
```

### Preview Mode

When AI suggests changes, viewport shows:
- Ghost overlay of proposed geometry (semi-transparent)
- Dimension annotations showing what changes
- Before/after comparison toggle
- Accept/Reject buttons in viewport

### Interaction States

| State | Behavior |
|-------|----------|
| Idle | Chat panel minimized, icon in toolbar |
| Active | Panel open, AI ready for questions |
| Analyzing | Spinner, "Analyzing design..." message |
| Suggesting | Options displayed with preview buttons |
| Previewing | Ghost geometry in viewport, accept/reject UI |
| Applying | Change being executed, progress indicator |

### Error Handling

| Condition | Response |
|-----------|----------|
| Ambiguous reference | "Which part do you mean? [Part A] [Part B]" |
| Impossible request | Explain why, suggest alternatives |
| Simulation timeout | "This simulation is taking long. [Continue] [Cancel]" |
| No solution found | "I couldn't find a way to achieve that. Here's why..." |

## Implementation

### Files to Create

| File | Purpose |
|------|---------|
| `packages/mcp/src/tools/measure.ts` | Distance, angle, clearance tools |
| `packages/mcp/src/tools/analyze.ts` | Workspace, mass properties, constraints |
| `packages/mcp/src/tools/simulate.ts` | Motion simulation tools |
| `packages/mcp/src/tools/modify.ts` | Parameter modification tools |
| `packages/mcp/src/context/document.ts` | Document context extraction |
| `packages/app/src/components/AIChatPanel.tsx` | Chat UI component |
| `packages/app/src/components/AIPreview.tsx` | Suggestion preview overlay |
| `packages/app/src/components/AISuggestionCard.tsx` | Option card component |
| `packages/core/src/stores/ai-store.ts` | AI conversation state |

### Files to Modify

| File | Changes |
|------|---------|
| `packages/mcp/src/server.ts` | Register new tools |
| `packages/app/src/App.tsx` | Add AIChatPanel |
| `packages/app/src/components/Toolbar.tsx` | Add AI toggle button |
| `packages/core/src/stores/ui-store.ts` | Add AI panel visibility state |

### Data Structures

**Conversation State:**

```typescript
interface AIConversation {
  id: string;
  messages: AIMessage[];
  documentSnapshot: string;  // Document hash when conversation started
  activeSuggestions: AISuggestion[];
}

interface AIMessage {
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
  toolCalls?: ToolCall[];
  suggestions?: AISuggestion[];
}

interface AISuggestion {
  id: string;
  title: string;
  description: string;
  impact: SuggestionImpact;
  operations: CsgOp[];  // Operations to apply
  previewMesh?: Float32Array;  // Pre-computed preview geometry
}

interface SuggestionImpact {
  massChange?: number;      // Percentage
  strengthChange?: number;  // Percentage (safety factor)
  reachChange?: number;     // mm
  torqueChange?: number;    // Percentage
  costChange?: number;      // Percentage (if materials involved)
}
```

**Tool Schemas:**

```typescript
// measure_distance
interface MeasureDistanceInput {
  from: EntityReference;  // { type: 'vertex' | 'edge' | 'face', partId: string, index: number }
  to: EntityReference;
  mode: 'minimum' | 'center-to-center' | 'along-axis';
}

interface MeasureDistanceOutput {
  distance: number;  // mm
  fromPoint: [number, number, number];
  toPoint: [number, number, number];
  direction: [number, number, number];  // Unit vector
}

// get_workspace
interface GetWorkspaceInput {
  mechanismRoot: string;  // Part ID
  endEffector: string;    // Part ID
  resolution: number;     // Points per axis (default: 20)
}

interface GetWorkspaceOutput {
  reachablePoints: [number, number, number][];
  boundingBox: { min: [number, number, number]; max: [number, number, number] };
  volume: number;  // mm^3
  singularityRegions: [number, number, number][];  // Points near singularities
}
```

### Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Browser                              │
│  ┌──────────────────┐  ┌──────────────────────────────────┐ │
│  │  AIChatPanel.tsx │  │  Three.js Viewport               │ │
│  │                  │  │  ┌─────────────────────────────┐ │ │
│  │  [Messages]      │  │  │  AIPreview.tsx              │ │ │
│  │  [Input]         │  │  │  (ghost geometry overlay)   │ │ │
│  │  [Suggestions]   │  │  └─────────────────────────────┘ │ │
│  └────────┬─────────┘  └──────────────────────────────────┘ │
│           │                                                 │
│           ▼                                                 │
│  ┌──────────────────┐                                       │
│  │   ai-store.ts    │                                       │
│  │   (Zustand)      │                                       │
│  └────────┬─────────┘                                       │
└───────────┼─────────────────────────────────────────────────┘
            │ HTTP/WebSocket
            ▼
┌───────────────────────────────────────────────────────────┐
│                    MCP Server                              │
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐       │
│  │ measure.ts  │  │ analyze.ts  │  │ simulate.ts │       │
│  └─────────────┘  └─────────────┘  └─────────────┘       │
│           │               │               │               │
│           └───────────────┴───────────────┘               │
│                           │                               │
│                           ▼                               │
│                  ┌─────────────────┐                      │
│                  │ @vcad/engine    │                      │
│                  │ (WASM kernel)   │                      │
│                  └─────────────────┘                      │
└───────────────────────────────────────────────────────────┘
```

## Tasks

### Phase 1: Measurement Tools (Week 1-2)

- [ ] Implement `measure_distance` with all modes (`m`)
- [ ] Implement `measure_angle` for edges and faces (`s`)
- [x] Implement `check_clearance` between parts (`m`)
- [ ] Implement `get_mass_properties` (`s`)
- [ ] Add entity reference resolution from selection (`s`)
- [ ] Write unit tests for measurement accuracy (`s`)

### Phase 2: Analysis Tools (Week 2-3)

- [ ] Implement `get_workspace` for articulated mechanisms (`l`)
- [ ] Implement `analyze_constraints` with DOF calculation (`m`)
- [ ] Implement `get_joint_limits` from document (`s`)
- [ ] Implement `find_singularities` for kinematic chains (`m`)
- [ ] Cache analysis results for performance (`s`)

### Phase 3: Simulation Tools (Week 3-4)

- [ ] Implement `simulate_motion` with trajectory output (`m`)
- [ ] Implement `compute_torques` via inverse dynamics (`m`)
- [ ] Implement `check_interference` through motion (`m`)
- [ ] Add timeout and progress reporting (`s`)
- [ ] Integrate with physics simulation store (`s`)

### Phase 4: Modification Tools (Week 4-5)

- [ ] Implement `modify_dimension` with validation (`m`)
- [ ] Implement `apply_suggestion` for atomic changes (`s`)
- [ ] Implement `create_variant` for comparison (`s`)
- [ ] Add preview mesh generation for suggestions (`m`)
- [ ] Ensure undo integration for all modifications (`s`)

### Phase 5: Chat UI (Week 5-6)

- [ ] Create `ai-store.ts` with conversation state (`s`)
- [ ] Build `AIChatPanel.tsx` with message display (`m`)
- [ ] Implement suggestion cards with preview buttons (`m`)
- [ ] Add `AIPreview.tsx` ghost geometry overlay (`m`)
- [ ] Wire up accept/reject for suggestions (`s`)
- [ ] Add conversation history persistence (`s`)

### Phase 6: Context & Polish (Week 6-8)

- [ ] Implement `get_document_context` for full state (`m`)
- [ ] Add selection-aware context injection (`s`)
- [ ] Implement streaming responses for long analysis (`m`)
- [ ] Add keyboard shortcuts (Cmd+Shift+A to toggle) (`xs`)
- [ ] Performance optimization for large documents (`m`)
- [ ] Write user documentation and examples (`s`)

## Acceptance Criteria

- [ ] AI can measure distances between any two entities with <0.1mm accuracy
- [ ] AI correctly identifies reachability issues and suggests fixes
- [ ] Simulation-based diagnosis identifies root cause of failures
- [ ] Suggestions include quantified tradeoffs (mass, torque, strength)
- [ ] Preview shows proposed changes before applying
- [ ] Changes apply atomically with undo support
- [ ] Conversation context maintained across multiple turns
- [ ] Response time <3s for most queries, <10s for simulations
- [ ] AI correctly identifies design issues >80% of the time
- [ ] Works with assemblies up to 50 parts

## Success Metrics

| Metric | Target |
|--------|--------|
| Issue identification accuracy | >80% |
| Suggestion actionability | >90% one-click applicable |
| User acceptance rate | >60% of suggestions accepted |
| Context retention | 100% across conversation |
| Response latency (P90) | <5s |

## Competitive Advantage

No CAD tool has AI that truly understands the design:

| Capability | vcad AI Co-Designer | Fusion 360 | SolidWorks | Onshape |
|------------|---------------------|------------|------------|---------|
| Natural language queries | Yes | No | No | Limited |
| Geometry understanding | Full BRep access | None | None | None |
| Physics reasoning | Integrated simulation | Separate tool | Separate tool | No |
| Tradeoff analysis | Quantified options | Manual | Manual | Manual |
| One-click fixes | Yes | No | No | No |
| Open source | Yes | No | No | No |

### Why This Matters

1. **10x faster iteration** — AI does analysis that takes engineers hours
2. **Catches issues early** — Problems identified before manufacturing
3. **Democratizes expertise** — Junior engineers get senior-level insights
4. **Continuous learning** — AI improves from every interaction
5. **Seamless workflow** — No context switching between tools

## Future Enhancements

- [ ] Multi-modal input (sketch on screen → AI interprets)
- [ ] Design rules enforcement ("check ASME standards")
- [ ] Collaborative mode (AI mediates between multiple designers)
- [ ] Learning from user preferences (style transfer)
- [ ] Manufacturing feedback ("this will be hard to machine")
- [ ] Cost optimization with supplier data
- [ ] Generative design proposals
- [ ] Voice interface for hands-free operation
