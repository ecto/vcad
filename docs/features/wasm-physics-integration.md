# WASM Physics Integration

**Score: 80/100** | **Priority: #10** | **Status: P0 Roadmap**

## Overview

Compile vcad-kernel-physics (phyz) to WASM, enabling browser-based physics simulation. This is the foundation for all browser simulation features including robot control, physics-always-on UX, and multiplayer physics synchronization.

## Current State

| Component | Status |
|-----------|--------|
| vcad-kernel-physics | ✅ Complete (~1000 LOC) |
| phyz 0.23 integration | ✅ Complete |
| Joint types (5) | ✅ Revolute, Prismatic, Fixed, Spherical, Generic |
| Gym-style RobotEnv | ✅ Complete |
| WASM compilation | ❌ Not started |

The native Rust implementation is fully functional with a gym-style interface for reinforcement learning. The physics crate integrates with the existing vcad-kernel ecosystem but has not yet been exposed to the browser.

## Technical Implementation

### 1. Feature Flag in vcad-kernel-wasm

Add a `physics` feature to `vcad-kernel-wasm/Cargo.toml`:

```toml
[features]
default = []
physics = ["vcad-kernel-physics"]

[dependencies]
vcad-kernel-physics = { path = "../vcad-kernel-physics", optional = true }
```

### 2. phyz WASM Compatibility

phyz is pure Rust with no platform-specific dependencies, so it compiles to
WASM without special configuration:

```toml
[dependencies]
phyz = { path = "../../../phyz/crates/phyz", version = "0.2" }
```

Key considerations:
- phyz uses `tang-la` for linear algebra, which is `no_std`-friendly
- Featherstone ABA is single-threaded; phyz-gpu provides batched WGPU
  kernels if browser parallelism is needed

### 3. WASM Bindings

Expose core types via wasm-bindgen in `vcad-kernel-wasm/src/physics.rs`:

```rust
#[wasm_bindgen]
pub struct PhysicsWorld {
    inner: vcad_kernel_physics::PhysicsWorld,
}

#[wasm_bindgen]
impl PhysicsWorld {
    #[wasm_bindgen(constructor)]
    pub fn from_document(doc: &JsValue) -> Result<PhysicsWorld, JsError> {
        let ir: IrDocument = serde_wasm_bindgen::from_value(doc.clone())?;
        let inner = vcad_kernel_physics::PhysicsWorld::from_ir(&ir)?;
        Ok(Self { inner })
    }

    pub fn step(&mut self, dt: f64) {
        self.inner.step(dt);
    }

    pub fn set_joint_position(&mut self, joint_id: &str, value: f64) {
        self.inner.set_joint_position(joint_id, value);
    }

    pub fn get_joint_positions(&self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.inner.joint_positions()).unwrap()
    }
}
```

### 4. TypeScript Bindings

Add types to `packages/kernel-wasm/src/index.ts`:

```typescript
export interface PhysicsWorld {
  step(dt: number): void;
  setJointPosition(jointId: string, angle: number): void;
  setJointVelocity(jointId: string, velocity: number): void;
  applyJointTorque(jointId: string, torque: number): void;
  getJointPositions(): Map<string, number>;
  getEndEffectorPoses(effectorIds: string[]): Map<string, Pose>;
}

export function createPhysicsWorld(document: IrDocument): PhysicsWorld;
```

### 5. Web Worker Integration

Run physics in a dedicated worker to avoid blocking the main thread:

```typescript
// packages/engine/src/physics-worker.ts
import init, { PhysicsWorld } from '@vcad/kernel-wasm';

let world: PhysicsWorld | null = null;

self.onmessage = async (e) => {
  const { type, payload } = e.data;

  switch (type) {
    case 'init':
      await init();
      world = new PhysicsWorld(payload.document);
      break;
    case 'step':
      world?.step(payload.dt);
      self.postMessage({ type: 'state', payload: world?.getJointPositions() });
      break;
    case 'control':
      world?.setJointPosition(payload.jointId, payload.value);
      break;
  }
};
```

## API Surface

### Core API

```typescript
// Create physics world from document
const world = kernel.createPhysicsWorld(document);

// Step simulation
world.step(dt);

// Control joints
world.setJointPosition(jointId, angle);
world.setJointVelocity(jointId, velocity);
world.applyJointTorque(jointId, torque);

// Get state
const positions = world.getJointPositions();
const poses = world.getEndEffectorPoses(effectorIds);
```

### Worker API (Recommended)

```typescript
const physics = new PhysicsWorker();
await physics.init(document);

// Start continuous simulation
physics.start({ dt: 1/240, substeps: 4 });

// Subscribe to state updates
physics.onState((state) => {
  updateVisualization(state.positions);
});

// Send control commands
physics.setJointPosition('joint_1', Math.PI / 4);
```

## Challenges

### SIMD Browser Support

phyz uses SIMD for performance. Mitigation:
- Build two WASM binaries: `physics.wasm` (SIMD) and `physics-fallback.wasm` (scalar)
- Feature-detect at runtime: `WebAssembly.validate(simdTestBytes)`
- Load appropriate binary

### Memory Management

WASM linear memory requires careful handling:
- Pre-allocate physics world memory based on body count estimate
- Use typed arrays for bulk state transfer (positions, velocities)
- Avoid per-frame allocations in hot paths
- Implement explicit `dispose()` for cleanup

### Determinism

Physics replay and debugging require deterministic simulation:
- Use fixed timestep (1/240s recommended)
- Disable parallel solver when determinism required
- Serialize full world state for snapshots
- Document floating-point caveats across browsers

## Success Metrics

| Metric | Target |
|--------|--------|
| Physics step latency | <2ms for <50 bodies |
| Memory overhead | <10MB base + ~100KB/body |
| Browser support | Chrome 90+, Firefox 90+, Safari 15+ |
| Behavior parity | Identical to native Rust (within FP tolerance) |

### Performance Benchmarks

Run automated benchmarks on CI:
- 10 bodies: <0.5ms/step
- 50 bodies: <2ms/step
- 100 bodies: <5ms/step

## Unlocks

This feature enables:

1. **Browser-based robot simulation** - Design and test robots entirely in the browser
2. **Physics-always-on UX** - Real-time gravity, collisions, and constraints during modeling
3. **MCP gym tools with real physics** - `gym_step`, `gym_reset`, `gym_observe` use actual phyz
4. **Multiplayer physics** - Shared deterministic state across clients
5. **Training data generation** - Collect trajectories for ML in browser

## Dependencies

**None** - This feature can start immediately.

### Related Features

- Browser Robot Simulation (depends on this)
- Physics-Always-On UX (depends on this)
- Collision Detection (partial overlap)

## Implementation Plan

### Phase 1: Basic WASM Build (1-2 days)
- Add physics feature flag
- Configure phyz for WASM target
- Verify compilation succeeds

### Phase 2: Bindings (2-3 days)
- Implement wasm-bindgen exports
- Add TypeScript types
- Basic integration test

### Phase 3: Worker Integration (2-3 days)
- Web Worker wrapper
- State synchronization
- Performance profiling

### Phase 4: Polish (1-2 days)
- SIMD fallback
- Memory optimization
- Documentation

**Estimated Total: 6-10 days**
