# wasmosis

Lazy WASM module splitting with automatic inference.

## Overview

wasmosis splits large WASM binaries into lazy-loadable modules. Functions are automatically assigned to modules based on feature gates and dependencies - no manual annotation required.

## Quick Start

### 1. Analyze Your Crate

```bash
wasmosis analyze src/lib.rs --show-inference
```

```
Core module (47 functions):
    - cube [default]
    - cylinder [default]

Secondary modules:
  physics (12 functions):
    - is_physics_available [feature-gate]
    - create_physics_env [dependency: vcad_kernel_physics]

  gpu (9 functions):
    - init_gpu [feature-gate]

Inference Summary:
  default: 47 functions
  feature-gate: 16 functions
  explicit: 4 functions
```

### 2. Generate Split Crates

```bash
wasmosis codegen src/lib.rs \
  --out-dir ./generated \
  --base-name my-kernel \
  --ts-out-dir ./loader
```

### 3. Build

```bash
wasmosis build \
  --crates-dir ./generated \
  --out-dir ./dist \
  --target web
```

## How Inference Works

Functions are assigned to modules in priority order:

### 1. Explicit Annotation

```rust
#[module("custom")]
#[wasm_bindgen]
pub fn my_function() { }
```

### 2. Feature Gates

```rust
#[cfg(feature = "physics")]
#[wasm_bindgen]
pub fn is_physics_available() -> bool { true }
// → "physics" module
```

### 3. Dependency Detection

```rust
#[wasm_bindgen]
pub fn create_env(doc: &str) -> PhysicsSim {
    vcad_kernel_physics::RobotEnv::new(doc)
}
// → "physics" module (detected from crate path)
```

### 4. Default

No trigger → `core` module (always loaded).

## Dependency Mapping

| Crate Pattern | Module |
|---------------|--------|
| `vcad_kernel_physics` | physics |
| `vcad_kernel_gpu` | gpu |
| `vcad_kernel_raytrace` | raytrace |
| `vcad_slicer` | slicer |
| `vcad_kernel_cam` | cam |
| `vcad_kernel_drafting` | drafting |
| `stepperoni` | step |

## CLI Reference

### analyze

```bash
wasmosis analyze <input.rs|input.wasm> [options]

Options:
  --json            Output as JSON
  --show-inference  Show inference reasoning
```

### codegen

```bash
wasmosis codegen <input.rs> [options]

Options:
  -o, --out-dir <dir>    Output directory (default: ./generated)
  -n, --base-name <name> Base crate name (default: wasm-module)
  -t, --ts-out-dir <dir> TypeScript loader output
  -d, --deps <file>      Dependencies JSON file
```

### build

```bash
wasmosis build [options]

Options:
  -c, --crates-dir <dir> Crates directory (default: ./generated)
  -o, --out-dir <dir>    Output directory (default: ./dist)
  -t, --target <target>  web | bundler | nodejs (default: web)
  --dev                  Dev mode (default: release)
```

## TypeScript API

### Generated Kernel

```typescript
import { Kernel } from './loader';
import type { WasmMesh } from './loader';

const kernel = await Kernel.init();

// Core (sync, always loaded)
const solid = kernel.cube(10, 10, 10);
const mesh: WasmMesh = kernel.getMesh(solid);

// Lazy (async, loaded on first use)
const available = await kernel.isPhysicsAvailable();
```

### ts-rs Integration

```rust
#[derive(Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "generated/"))]
pub struct WasmMesh {
    pub positions: Vec<f32>,
    pub indices: Vec<u32>,
}
```

```bash
cargo test --features ts-rs -- export_bindings --ignored
```

## Programmatic API

```typescript
import {
  parseRustSourceWithInference,
  generateCrates,
  generateTypeScriptLoader,
} from 'wasmosis';

const parseResult = parseRustSourceWithInference('src/lib.rs');

const crates = generateCrates(parseResult, {
  baseName: 'my-kernel',
  outDir: './generated',
  dependencies: { 'vcad-kernel': { version: '0.8' } },
});

generateTypeScriptLoader(crates, {
  packageName: '@my-org/kernel',
  outDir: './loader',
  wasmBasePath: './',
  typeSafe: true,
});
```
