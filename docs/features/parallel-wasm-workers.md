# Parallel WASM Workers

**Score: 81/100** | **Priority: #9**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. Shipped: a background eval worker with its own WASM instance (`packages/engine/src/index.ts` → `eval-worker.js`, pre-compiled module handoff), an analyze worker (`packages/engine/src/analyze.ts`), and a slicer worker (`packages/app/src/lib/slicer-client.ts`). Not shipped: the multi-worker pool, SharedArrayBuffer zero-copy sharing, and cross-origin-isolation setup described below.

## Overview

Rust's fearless concurrency combined with Web Workers enables parallel CAD operations without blocking the UI. Heavy compute (booleans, tessellation, physics) runs in background workers while the main thread maintains 60fps rendering.

## Why It Matters

| Problem | Impact |
|---------|--------|
| Boolean operations | 100ms+ on complex models |
| Physics simulation | Needs consistent timesteps |
| Multi-part tessellation | Embarrassingly parallel |
| Main thread blocking | Drops below 60fps, poor UX |

Most CAD tools block the UI during compute. Parallel workers give vcad a responsiveness advantage.

## Technical Implementation

### Worker Pool

```typescript
class WasmWorkerPool {
  private workers: Worker[];
  private queue: Task[];

  constructor() {
    const count = Math.min(navigator.hardwareConcurrency || 4, 8);
    this.workers = Array.from({ length: count }, () =>
      new Worker(new URL('./kernel-worker.ts', import.meta.url))
    );
  }
}
```

### Key Technologies

- **Worker pool**: 4-8 workers based on `navigator.hardwareConcurrency`
- **Independent WASM instances**: Each worker loads its own kernel
- **SharedArrayBuffer**: Zero-copy data sharing for large buffers
- **Transferable objects**: Move ownership of ArrayBuffers without copying
- **Message passing**: Structured operation dispatch and results

### Memory Architecture

```
┌─────────────────────────────────────────────────┐
│              SharedArrayBuffer                  │
│  (vertex data, indices, shared geometry)        │
└─────────────────────────────────────────────────┘
        ▲           ▲           ▲           ▲
        │           │           │           │
   ┌────┴──┐   ┌────┴──┐   ┌────┴──┐   ┌────┴──┐
   │Worker1│   │Worker2│   │Worker3│   │Worker4│
   │ WASM  │   │ WASM  │   │ WASM  │   │ WASM  │
   └───────┘   └───────┘   └───────┘   └───────┘
```

## Architecture

```
Main Thread              Worker Pool
    │                   ┌─────────────┐
    │──tessellate A────▶│  Worker 1   │
    │──tessellate B────▶│  Worker 2   │
    │──physics step────▶│  Worker 3   │
    │                   │  Worker 4   │
    │◀──results─────────└─────────────┘
    │
   UI (always responsive)
```

## Parallelizable Operations

| Operation | Parallelization Strategy |
|-----------|-------------------------|
| Tessellation | Per-part parallel |
| Booleans | Independent ops parallel |
| Physics | Dedicated background worker |
| STEP export | Separate worker |
| Constraint solving | Parallel across independent sketches |
| Ray casting | Batch queries across workers |

### Operation Dispatch

```typescript
interface WorkerTask {
  id: string;
  type: 'tessellate' | 'boolean' | 'physics' | 'export';
  payload: Transferable[];
  priority: number;
}

// Dispatch to least-loaded worker
pool.dispatch({
  type: 'tessellate',
  payload: [partBuffer],
  priority: 1
});
```

## Implementation Details

### Worker Lifecycle

1. **Initialization**: Load WASM module, allocate memory
2. **Idle**: Wait for tasks in pool
3. **Processing**: Execute operation, post results
4. **Cleanup**: Release resources on termination

### Error Handling

```typescript
worker.onerror = (e) => {
  console.error(`Worker ${id} failed:`, e);
  pool.respawn(id); // Replace failed worker
  pool.requeue(task); // Retry on another worker
};
```

### Cross-Origin Isolation

SharedArrayBuffer requires specific headers:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

Fallback to Transferable-only mode when headers unavailable.

## Success Metrics

| Metric | Target |
|--------|--------|
| UI frame budget | Never block >16ms |
| Batch speedup | 4x on 4-core machines |
| Physics timestep | Consistent dt regardless of UI |
| Worker startup | <100ms warm, <500ms cold |
| Memory overhead | <50MB per worker |

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| Memory overhead per worker | Lazy WASM init, shared read-only data |
| Cross-origin header requirements | Feature detection, graceful fallback |
| Worker communication overhead | Batch small operations, use Transferables |
| Debugging complexity | Structured logging, worker IDs in traces |

## Related Features

- **vcad-kernel-wasm**: WASM bindings that workers load
- **Physics simulation**: Primary consumer of background worker
- **Batch export**: Parallel STL/STEP generation

## References

- [MDN Web Workers API](https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API)
- [SharedArrayBuffer](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer)
- [wasm-bindgen Parallel Rayon](https://rustwasm.github.io/wasm-bindgen/examples/raytrace.html)
