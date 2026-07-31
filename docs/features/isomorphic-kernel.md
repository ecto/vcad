# Isomorphic Kernel

**Score: 85/100** | **Priority: #5**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. WASM (`crates/vcad-kernel-wasm` → browser) and native (`vcad-cli`, desktop) targets ship from the single Rust workspace. The WASI (`wasm32-wasi`) target is aspirational — no wasm32-wasi build exists in the workspace or CI, and no edge deployment ships.

## Overview

The vcad kernel is written in Rust and compiles to multiple targets from a single codebase:

- **WASM** (wasm32-unknown-unknown) — browser via web app
- **Native** (x86_64-apple-darwin, x86_64-unknown-linux-gnu, etc.) — CLI and desktop
- **WASI** (wasm32-wasi) — edge compute and serverless

Identical geometry algorithms run everywhere. A boolean operation in the browser produces the exact same result as the CLI or a Cloudflare Worker.

## Why It Matters

**No platform-specific bugs.** When geometry code is shared, there's no "works in browser but fails in CLI" class of bugs. A fix in one place fixes all platforms.

**Server-side rendering uses production code.** Thumbnails, previews, and exports generated server-side use the same kernel as interactive editing. No divergence.

**Edge compute becomes viable.** WASI support enables running the kernel in Cloudflare Workers, Deno Deploy, or Fastly Compute for low-latency thumbnail generation and file conversion at the edge.

**One codebase to maintain.** Engineering effort compounds instead of fragmenting across platform-specific implementations.

## Technical Implementation

### Workspace Structure

```
crates/
├── vcad-kernel-math/       # Linear algebra, transforms
├── vcad-kernel-topo/       # Half-edge BRep topology
├── vcad-kernel-geom/       # Curves and surfaces
├── vcad-kernel-booleans/   # Boolean operations
├── vcad-kernel/            # Unified API (platform-agnostic)
├── vcad-kernel-wasm/       # WASM bindings (browser)
└── vcad-cli/               # Native CLI
```

Core crates (`vcad-kernel-*`) contain zero platform-specific code. All I/O, threading, and system calls live in leaf crates that depend on the core.

### Compile Targets

| Target | Use Case | Build Command |
|--------|----------|---------------|
| `wasm32-unknown-unknown` | Browser | `wasm-pack build` |
| `x86_64-apple-darwin` | macOS CLI/desktop | `cargo build --release` |
| `x86_64-unknown-linux-gnu` | Linux CLI/server | `cargo build --release` |
| `aarch64-apple-darwin` | Apple Silicon | `cargo build --release` |
| `wasm32-wasi` | Edge functions | `cargo build --target wasm32-wasi` |

### Feature Flags

Platform-specific functionality uses Cargo features:

```toml
[features]
default = []
native-io = ["std"]      # File system access
parallel = ["rayon"]     # Multi-threaded tessellation
wasi = []                # WASI-specific bindings
```

Core geometry code has no features — it's pure computation.

### Shared Test Suite

```bash
# Run on native
cargo test --workspace

# Run on WASM (via wasm-pack)
wasm-pack test --node crates/vcad-kernel-wasm

# Run on WASI
cargo test --target wasm32-wasi --workspace
```

The same test assertions validate behavior across all targets.

## Use Cases

### Browser — Interactive Editing

The web app loads the WASM kernel for real-time boolean operations, constraint solving, and tessellation. Users get native-speed geometry processing in the browser.

### CLI — Batch Processing

```bash
vcad export model.vcad output.stl
vcad import-step assembly.step output.vcad
```

CI/CD pipelines validate models, generate exports, and run geometry checks using the native CLI.

### Edge — Thumbnail Generation

```
Request → Cloudflare Worker (WASI kernel) → PNG thumbnail
```

Sub-100ms latency for generating previews close to users. No round-trip to origin servers.

### Server — Headless Rendering

API endpoints for file conversion, geometry validation, and batch exports. Same kernel as the browser ensures consistent results.

## Architecture

```
                    vcad-kernel (Rust)
                          │
          ┌───────────────┼───────────────┐
          │               │               │
          ▼               ▼               ▼
   wasm32-unknown    native targets    wasm32-wasi
      │                   │               │
      ▼                   ▼               ▼
   Browser            CLI/Desktop    Edge Functions
   (web app)          (vcad-cli)     (Workers, Deno)
```

Data flows through the same geometry pipeline regardless of target. Only I/O adapters differ.

## Success Metrics

| Metric | Target |
|--------|--------|
| Test parity | 100% of tests pass on all targets |
| Geometry consistency | Bit-identical output where floating-point allows |
| Export equivalence | CLI and browser produce identical STL/STEP files |
| Edge latency | < 100ms for thumbnail generation |

## Competitive Advantage

Most CAD tools maintain separate codebases:

- **Onshape**: Server-side Parasolid kernel, separate browser viewer
- **Fusion 360**: Desktop kernel, web viewer is display-only
- **FreeCAD**: Desktop-only, no browser support

vcad's isomorphic approach means:

- Features ship to all platforms simultaneously
- Bug fixes propagate everywhere
- No "web version is limited" compromises
- Edge deployment without reimplementation

Single-source geometry is a structural advantage that compounds over time.
