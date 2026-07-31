# Zero-Latency Parametric Editing

**Score: 91/100** | **Priority: #1**

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |

> Verified against repo 2026-07-30. State row added. The core architecture is shipped: the full BRep kernel runs client-side via `vcad-kernel-wasm` / `@vcad/engine` (workers exist for evaluation and analysis: `packages/engine/src/eval-worker.ts`, `analyze-worker.ts`). Specific performance numbers below are targets, not measured guarantees.

## Overview

Zero-latency parametric editing enables all geometry operations to run client-side in WebAssembly at near-native speed. Boolean operations complete in 2-5ms, enabling 60fps parametric editing with no server round-trips.

When a user drags a dimension slider, the geometry updates instantly. No spinners. No waiting. The full BRep kernel runs in the browser, making parametric CAD feel like direct manipulation rather than command-execute-wait.

## Why It Matters

### The Legacy Desktop Era
SolidWorks, CATIA, and other traditional CAD tools run on desktop with C++ kernels. They're fast, but locked to specific platforms and expensive licenses.

### The Cloud CAD Tradeoff
Fusion 360 and Onshape moved their kernels to the cloud for cross-platform access. The tradeoff: every geometry operation requires a server round-trip. Edit a dimension? Wait 200-500ms for the server. On poor connections, this becomes seconds.

### The vcad Approach
vcad compiles a full Rust BRep kernel to WebAssembly. The kernel runs entirely in the browser:

- **No server dependency** for geometry operations
- **Consistent performance** regardless of network conditions
- **True cross-platform** with native-like speed
- **Works offline** once loaded

## Technical Implementation

### Architecture

```
User Input → Store Update → WASM Kernel → Tessellation → Three.js Render
   0ms          <1ms          2-5ms          1-2ms          8-10ms
                              ↑
                        No network hop
```

### WASM Compilation Pipeline

1. **Rust kernel crates** (`vcad-kernel-*`) implement BRep topology, booleans, and tessellation
2. **wasm-pack** compiles to WebAssembly with size optimizations
3. **wasm-bindgen** generates TypeScript bindings for seamless JS interop
4. **Engine package** wraps WASM module with async initialization and memory management

### Performance Targets

| Operation | Target | Typical Model |
|-----------|--------|---------------|
| Boolean union/difference | <5ms | 1-5K triangles |
| Parametric rebuild | <10ms | 10-parameter part |
| Tessellation | <3ms | 10K triangles |
| Full update cycle | <16ms | Slider drag → render |

### Key Optimizations

- **Arena-based allocation** via `slotmap` minimizes memory churn
- **AABB broadphase** filters boolean candidates before expensive intersection
- **Incremental tessellation** only re-meshes affected faces
- **SharedArrayBuffer** enables zero-copy data transfer to render thread

## User Experience

### What Users See

**Dimension editing**: Drag a slider, geometry morphs smoothly at 60fps. No "Rebuilding..." dialog.

**Boolean preview**: Hover a subtract operation, see the result instantly. Commit or cancel with no delay.

**Constraint solving**: Move a sketch point, all constrained geometry updates in the same frame.

**Assembly manipulation**: Drag a joint, kinematic chain follows in real-time.

### What Users Don't See

- Network latency
- Server queue times
- "Processing..." spinners
- Stale geometry while waiting for updates

### Comparison

| Action | Cloud CAD | vcad |
|--------|-----------|------|
| Edit dimension | 200-500ms | <16ms |
| Boolean operation | 300-800ms | 2-5ms |
| Regenerate model | 500ms-2s | <50ms |
| Work offline | No | Yes |

## Success Metrics

### Performance

- Boolean operations <10ms for meshes under 10K triangles
- Parametric update cycle maintains 60fps (16ms budget)
- WASM module size <2MB gzipped for fast initial load

### User Impact

- Zero network requests for geometry operations
- No perceptible delay between input and visual feedback
- Offline-capable after initial page load

### Technical

- Memory usage <500MB for typical assemblies (100 parts)
- No main thread blocking during heavy operations
- Graceful degradation on lower-end hardware

## Dependencies

| Dependency | Status | Notes |
|------------|--------|-------|
| Rust BRep kernel | Complete | `vcad-kernel-*` crates |
| WASM compilation | Complete | `vcad-kernel-wasm` |
| JS engine wrapper | Complete | `@vcad/engine` |
| Zustand state management | Complete | Reactive updates |
| Three.js rendering | Complete | React Three Fiber |

The core infrastructure exists. This feature is about optimization and polish to hit performance targets consistently.

## Competitive Advantage

Cloud-based CAD systems face a fundamental constraint: network latency. Even with edge servers and optimized protocols, a round-trip to a remote kernel adds 100-500ms of unavoidable delay.

vcad eliminates this entirely. The kernel runs where the user is—in their browser. Performance depends only on local hardware, which continues to improve.

This advantage is structural, not temporary. Cloud CAD cannot match local execution latency without moving the kernel client-side—which means adopting the same architecture vcad already has.

### Market Positioning

- **vs. Desktop CAD**: Same performance, but cross-platform and web-native
- **vs. Cloud CAD**: Dramatically lower latency, works offline
- **vs. Both**: Open source, no vendor lock-in

## Implementation Notes

### Current State

The WASM kernel handles primitives, booleans, transforms, and basic features. Performance is already competitive for simple models.

### Optimization Opportunities

1. **Web Workers**: Move kernel to worker thread, keep main thread free
2. **SIMD**: Enable WASM SIMD for vectorized math operations
3. **Incremental updates**: Track dirty state to minimize recalculation
4. **LOD tessellation**: Coarse mesh during drag, refine on release

### Risks

- Complex assemblies may exceed single-core performance limits
- Browser memory constraints on very large models
- Mobile performance varies significantly by device

Mitigations include progressive loading, view-dependent LOD, and clear system requirements.
