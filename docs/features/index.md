# vcad Feature Documentation

Product management source of truth for vcad features. See [CONTRIBUTING.md](./CONTRIBUTING.md) for how to write and maintain feature specs.

**Mission: Become the best free CAD of all time — better than SolidWorks, Parasolid, CATIA.**

---

## Strategic Vision Features

These are the category-defining capabilities that will make vcad legendary. Scored using a weighted algorithm optimizing for CAD impact, differentiation, user pull, defensibility, and feasibility.

**Scoring: (CAD×3) + (Diff×2.5) + (Pull×2) + (Moat×1.5) + (Feas×1) — Max 100**

### Architecture Advantages (The Moat)

Capabilities uniquely enabled by our Rust/WASM architecture that competitors cannot easily replicate.

| Rank | Score | Feature | Status | Spec |
|------|-------|---------|--------|------|
| 1 | **91** | [Zero-Latency Parametric Editing](./zero-latency-parametric-editing.md) | `planned` | 60fps editing, <5ms booleans, no server |
| 5 | **85** | [Isomorphic Kernel](./isomorphic-kernel.md) | `partial` | Same Rust → WASM, native, WASI |
| 7 | **83** | [Browser-Native CAD](./browser-native.md) | `shipped` | Full CAD in browser, no install |
| 8 | **82** | [Offline-First Privacy](./offline-first-privacy.md) | `shipped` | Geometry never leaves browser |
| 9 | **81** | [Parallel WASM Workers](./parallel-wasm-workers.md) | `planned` | Background compute, responsive UI |
| 10 | **80** | [WASM Physics Integration](./wasm-physics-integration.md) | `planned` | phyz in browser — **P0** |

### Physics-First CAD (The Paradigm Shift)

Redefining CAD as design in a physical world, not static geometry.

| Rank | Score | Feature | Status | Spec |
|------|-------|---------|--------|------|
| 2 | **89** | [Physics Always On](./physics-always-on.md) | `planned` | No "run simulation" button |
| 3 | **87** | [CAD ↔ Physics Round-Trip](./cad-physics-roundtrip.md) | `planned` | Change dimension → physics updates |
| 13 | **77** | [Time as Dimension](./time-as-dimension.md) | `planned` | Scrub simulation like video |
| 22 | **68** | [Parametric Time Machine](./parametric-time-machine.md) | `proposed` | Drag sliders → feel the physics |

### AI-Native Design (The Differentiator)

Intelligence that understands geometry, physics, and intent.

| Rank | Score | Feature | Status | Spec |
|------|-------|---------|--------|------|
| 4 | **86** | [Unified Canvas](./unified-canvas.md) | `planned` | Geometry + physics + chat in one |
| 6 | **84** | [Intent-Based Modeling](./intent-based-modeling.md) | `proposed` | "Hinge here" → system infers |
| 12 | **78** | [AI Co-Designer](./ai-co-designer.md) | `proposed` | AI that thinks with you |
| 15 | **75** | [10-Second Robot](./ten-second-robot.md) | `proposed` | "3-DOF arm" → simulating instantly |
| 19 | **71** | ["Why Did It Fail?" Button](./why-did-it-fail.md) | `proposed` | One-click failure diagnosis |
| 24 | **66** | [Fork Reality](./fork-reality.md) | `proposed` | Paste image → parametric model |

### Robotics & RL (The Market)

First-class robotics simulation and machine learning.

| Rank | Score | Feature | Status | Spec |
|------|-------|---------|--------|------|
| 20 | **70** | [Multiplayer Physics](./multiplayer-physics.md) | `proposed` | Real-time collaborative simulation |
| 23 | **67** | [One-Click RL Training](./one-click-rl-training.md) | `proposed` | `vcad train` — no Isaac setup |

### The End Game

Closing the loop from design to reality — and beyond.

| Rank | Score | Feature | Status | Spec |
|------|-------|---------|--------|------|
| 16 | **74** | [Living Spec](./living-spec.md) | `proposed` | .vcad = executable specification |
| 21 | **75** | [CAD-Native World Model](./cad-native-world-model.md) | `proposed` | Train world models on parametric CAD + physics |
| 29 | **61** | [Instant Physicality](./instant-physicality.md) | `proposed` | Click "Order Parts" → quote |

### Implementation Priority

**P0 (Now):**
1. [WASM Physics Integration](./wasm-physics-integration.md) — Foundation for browser simulation
2. [Zero-Latency Parametric Editing](./zero-latency-parametric-editing.md) — Core UX differentiator

**P1 (Next):**
3. [Physics Always On](./physics-always-on.md) — Paradigm shift
4. [CAD ↔ Physics Round-Trip](./cad-physics-roundtrip.md) — Unique capability
5. [Unified Canvas](./unified-canvas.md) — Single workspace

**P2 (After):**
6. [Intent-Based Modeling](./intent-based-modeling.md) — AI-native UX
7. [AI Co-Designer](./ai-co-designer.md) — Deep understanding
8. [Time as Dimension](./time-as-dimension.md) — Novel interaction

---

## Feature Index

### Core Modeling (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Primitives](./primitives.md) | `shipped` | p0 | Box, cylinder, sphere, cone |
| [Boolean Operations](./boolean-operations.md) | `shipped` | p0 | Union, difference, intersection |
| [Transforms](./transforms.md) | `shipped` | p0 | Translate, rotate, scale |
| [Patterns](./patterns.md) | `shipped` | p0 | Linear and circular arrays |
| [Shell Operation](./shell-operation.md) | `shipped` | p0 | Hollow out solids |

### Sketch System (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Sketch Mode](./sketch-mode.md) | `shipped` | p0 | 2D drawing with constraints |
| [Sketch Operations](./sketch-operations.md) | `shipped` | p0 | Extrude, revolve, sweep, loft |

### Surface Operations (Partial)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Fillets & Chamfers](./fillets-chamfers.md) | `partial` | p1 | Kernel done, needs UI |

### Assembly (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Assembly & Joints](./assembly-joints.md) | `shipped` | p0 | Parts, instances, 5 joint types, FK |

### Technical Drawing (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [2D Drafting](./drafting-2d.md) | `shipped` | p0 | Projections, dimensions, GD&T |

### Import/Export (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Import/Export](./import-export.md) | `shipped` | p0 | STEP, STL, GLB |
| [Headless API](./headless-api.md) | `shipped` | p1 | CLI, MCP server |

### Visualization (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Viewport & Camera](./viewport-camera.md) | `shipped` | p0 | 3D navigation, gizmo, snaps |
| [Ray Tracing](./ray-tracing.md) | `shipped` | p1 | Direct BRep rendering |
| [Materials](./materials.md) | `shipped` | p1 | PBR materials |

### UI & Interaction (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Bottom Toolbar](./toolbar.md) | `shipped` | p0 | Tab-based tools with auto-switch |
| [Selection](./selection.md) | `shipped` | p0 | Multi-part selection |
| [Command Palette](./command-palette.md) | `shipped` | p1 | Quick command access |
| [Undo/Redo](./undo-redo.md) | `shipped` | p0 | 50-step history |

### Data & Storage (Shipped)

| Feature | Status | Priority | Spec |
|---------|--------|----------|------|
| [Document Persistence](./document-persistence.md) | `shipped` | p0 | Offline-first .vcad files |

---

## Foundation (Critical)

Technical debt and infrastructure for major features.

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Technical Debt Cleanup](./technical-debt.md) | `shipped` | p0 | s | Safety fixes, module refactoring |
| [VCode Format](./vcode.md) | `proposed` | p0 | xs | Terse text format for AI/compression |

---

## AI-Native Features (Proposed)

Category-defining AI capabilities that no competitor offers.

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Text-to-CAD (cad0)](./text-to-cad.md) | `proposed` | p0 | xl | Natural language → parametric CAD |

---

## Robotics & Simulation (Planned)

Robotics interoperability and physics simulation.

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [URDF Import/Export](./urdf-support.md) | `proposed` | p1 | s | Standard robotics format |
| [Physics Simulation & Gym](./physics-simulation.md) | `planned` | p2 | l | phyz physics, RL training |

---

## Feature Tree UX Improvements (Proposed)

Make vcad's feature tree legendary — better than SolidWorks, Fusion, Onshape.

### Tier 1: Highest Impact

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Hover Preview](./hover-preview.md) | `proposed` | p1 | s | Hover → highlight in viewport |
| [Timeline Scrubber](./timeline-scrubber.md) | `proposed` | p1 | m | Scrub through feature history |

### Tier 2: Feels Magical

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Smart Auto-Names](./smart-auto-names.md) | `proposed` | p1 | s | "Ø5 Hole" not "Cut-Extrude1" |

### Tier 3: Dependency Awareness

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Dependency Visualization](./dependency-visualization.md) | `proposed` | p2 | m | Show "↓3" badges, warnings |

### Tier 4: Removes Friction

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Inline Parameter Editing](./inline-parameter-editing.md) | `proposed` | p2 | m | Edit params in tree |

### Tier 5: Power User Features

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Keyboard Navigation](./keyboard-navigation.md) | `proposed` | p2 | s | j/k vim-style navigation |
| [Search & Filter](./search-filter.md) | `proposed` | p2 | s | Type to filter features |
| [Drag-Drop Reordering](./drag-drop-reordering.md) | `proposed` | p3 | m | Reorder with impact preview |
| [Feature Grouping](./feature-grouping.md) | `proposed` | p3 | s | Organize into folders |

### Tier 6: AI Differentiator

| Feature | Status | Priority | Effort | Spec |
|---------|--------|----------|--------|------|
| [Health & Intelligence](./health-intelligence.md) | `proposed` | p3 | l | Warnings, dead features, hints |

---

## Implementation Priority

### Now (P0)

1. **[WASM Physics Integration](./wasm-physics-integration.md)** — Foundation for browser simulation
2. **[Zero-Latency Parametric Editing](./zero-latency-parametric-editing.md)** — Core UX differentiator
3. **[VCode Format](./vcode.md)** — Parser/serializer for cad0

### Next (P1)

4. **[Physics Always On](./physics-always-on.md)** — Paradigm shift
5. **[CAD ↔ Physics Round-Trip](./cad-physics-roundtrip.md)** — Unique capability
6. **[Unified Canvas](./unified-canvas.md)** — Single workspace
7. **[Text-to-CAD (cad0)](./text-to-cad.md)** — THE hero feature

### After (P2)

8. **[Intent-Based Modeling](./intent-based-modeling.md)** — AI-native UX
9. **[AI Co-Designer](./ai-co-designer.md)** — Deep understanding
10. **[Time as Dimension](./time-as-dimension.md)** — Novel interaction
11. **[Parallel WASM Workers](./parallel-wasm-workers.md)** — Background compute

### Done

- ~~[Technical Debt Cleanup](./technical-debt.md)~~ — ✅ Shipped
- ~~[Browser-Native CAD](./browser-native.md)~~ — ✅ Shipped
- ~~[Offline-First Privacy](./offline-first-privacy.md)~~ — ✅ Shipped

### Future (Proposed)

**High-Impact Proposed Features:**
- **[10-Second Robot](./ten-second-robot.md)** — Instant robot generation
- **[Fork Reality](./fork-reality.md)** — Image → parametric model
- **[One-Click RL Training](./one-click-rl-training.md)** — `vcad train`
- **[Living Spec](./living-spec.md)** — Executable specifications
- **[Multiplayer Physics](./multiplayer-physics.md)** — Real-time collaboration
- **[Instant Physicality](./instant-physicality.md)** — Order parts in one click
- **[CAD-Native World Model](./cad-native-world-model.md)** — Foundation model training platform

**From [ROADMAP.md](./ROADMAP.md):**
- Point Cloud → CAD
- PCB Integration
- Topology Optimization

---

## Quick Stats

| Category | Shipped | Planned | Proposed |
|----------|---------|---------|----------|
| Core Modeling | 5 | 0 | 0 |
| Sketch System | 2 | 0 | 0 |
| Surface Ops | 0 | 0 | 1 |
| Assembly | 1 | 0 | 0 |
| Drafting | 1 | 0 | 0 |
| Import/Export | 2 | 0 | 0 |
| Visualization | 3 | 0 | 0 |
| UI/Interaction | 4 | 0 | 0 |
| Data/Storage | 1 | 0 | 0 |
| Foundation | 1 | 0 | 1 |
| Feature Tree UX | 0 | 0 | 10 |
| **Strategic Vision** | **2** | **7** | **13** |
| **Total** | **22** | **7** | **25** |

### Strategic Vision Breakdown

| Sub-Category | Shipped | Planned | Proposed |
|--------------|---------|---------|----------|
| Architecture Advantages | 2 | 4 | 0 |
| Physics-First CAD | 0 | 3 | 1 |
| AI-Native Design | 0 | 1 | 5 |
| Robotics & RL | 0 | 0 | 2 |
| The End Game | 0 | 0 | 3 |
