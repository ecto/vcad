# CLAUDE.md

Instructions for AI agents working on vcad.

## Overview

vcad is an open-source parametric CAD system aiming to replace Fusion 360, Onshape, and similar tools. It features a custom BRep kernel written in Rust, a React/Three.js web app, and AI-native interfaces via MCP.

**Live app:** https://vcad.io

## Prerequisites

vcad depends on the `tang` math workspace at a **sibling path** (`../tang`). Clone it
next to vcad before running `cargo build`:

```bash
git clone git@github.com:ecto/tang.git ../tang
```

Cargo paths in the workspace (`tang`, `tang-la`, `tang-expr`) all point at
`../tang/crates/*`.

**Fresh worktrees** (`.claude/worktrees/*`) start with no `node_modules` — run `npm ci`
before any npm/tauri command. Tauri needs the `cargo-tauri` binary (installed globally
via `cargo install tauri-cli`); the npm scripts invoke it as `cargo tauri`, so no local
`tauri` on PATH is required.

After `npm ci`, the app imports from `@vcad/core`, `@vcad/engine`, `@vcad/ir`, `@vcad/mcp`
which all resolve to `dist/index.js` — so workspace packages must be built before
`npm run dev` or `tauri:dev` will start. Fastest path:

```bash
VCAD_WASM_SKIP=1 npm run build --workspaces --if-present
```

`VCAD_WASM_SKIP=1` skips the wasm-pack rebuild when `packages/kernel-wasm/vcad_kernel_wasm*`
artifacts are already checked in; drop it if you need a fresh kernel WASM.

## Commands

```bash
# Rust
cargo test --workspace             # run all tests
cargo clippy --workspace -- -D warnings  # lint — must pass clean
cargo fmt --all --check            # formatting check
cargo build --workspace            # build everything

# TypeScript
npm ci                             # install deps
npm run build --workspaces         # build all TS packages
npm test --workspaces --if-present # run tests

# App
npm run dev -w @vcad/app           # run web app locally (browser)
npm run tauri:dev -w @vcad/app     # run desktop app (Tauri shell + Vite)

# Supabase (database)
supabase db push --dry-run         # preview migration changes
supabase db push                   # apply migrations to production
supabase db diff -f name           # generate migration from local changes
```

## Supabase

Cloud sync uses Supabase (Postgres + Auth). Config and migrations live in `supabase/`.

**Project:** `yteuhwciuxcbjwmabawj` (linked via `supabase link`)

**Tables:**
- `documents` — synced .vcad files (RLS: users own their docs)
- `document_versions` — automatic version history on content changes

**Adding a migration:**
1. Create `supabase/migrations/NNN_description.sql`
2. Test locally: `supabase db reset` (if running local Supabase)
3. Deploy: `supabase db push`

**Auth:** Google and GitHub OAuth configured in Supabase dashboard (not in config.toml to avoid secret leakage).

**Client:** `@vcad/auth` package wraps Supabase client. Sync logic in `packages/auth/src/sync.ts`.

## Architecture

```
vcad/
├── crates/                        # Rust workspace (~35K LOC)
│   ├── vcad-kernel-math/          # Linear algebra, transforms, exact predicates
│   ├── vcad-kernel-topo/          # Half-edge BRep topology
│   ├── vcad-kernel-geom/          # Curves and surfaces
│   ├── vcad-kernel-primitives/    # Box, cylinder, sphere, cone
│   ├── vcad-kernel-tessellate/    # BRep → triangle mesh
│   ├── vcad-kernel-booleans/      # Boolean operations (~5.4K LOC)
│   ├── vcad-kernel-nurbs/         # NURBS curves/surfaces
│   ├── vcad-kernel-fillet/        # Fillets and chamfers
│   ├── vcad-kernel-sketch/        # 2D sketch geometry
│   ├── vcad-kernel-constraints/   # Geometric constraint solver
│   ├── vcad-kernel-sweep/         # Sweep and loft operations
│   ├── vcad-kernel-shell/         # Shell and pattern ops
│   ├── vcad-kernel-step/          # STEP AP214 import/export
│   ├── vcad-kernel-drafting/      # 2D drawings, projections, GD&T
│   ├── vcad-kernel-gpu/           # wgpu compute shaders (normals, decimation)
│   ├── vcad-kernel-raytrace/      # Direct BRep ray tracing
│   ├── vcad-kernel-physics/       # Rapier3D physics simulation
│   ├── vcad-kernel-urdf/          # URDF robot description import
│   ├── vcad-kernel/               # Unified kernel API
│   ├── vcad-kernel-wasm/          # WASM bindings for browser
│   ├── vcad-ir/                   # Intermediate representation
│   ├── vcad-cli/                  # CLI tool
│   └── vcad/                      # Legacy CSG library (manifold-based)
│
│   # Work-in-progress crates (built but not yet wired to any consumer):
│   # - vcad-kernel-stocksim     # CAM stock sim (octree SDF, marching cubes); no consumers yet
├── packages/                      # TypeScript workspace
│   ├── app/                       # Web app (React + Three.js + Zustand)
│   ├── engine/                    # WASM engine wrapper + physics
│   ├── ir/                        # TypeScript IR types
│   ├── core/                      # Shared utilities and stores
│   ├── kernel-wasm/               # Kernel WASM package
│   ├── mcp/                       # MCP server for AI agents
│   ├── training/                  # ML training pipeline
│   └── docs/                      # Documentation site
├── supabase/                      # Database migrations and config
│   └── migrations/                # SQL migrations (pushed via `supabase db push`)
├── cad-lib/                       # Loon stdlib for vcad modeling (`.loon` source)
│   └── src/lib.loon               # bundled into crates/vcad-loon via include_str!
```

## Key Concepts

### BRep Kernel

The kernel uses **half-edge topology** (arena-based with `slotmap`) for boundary representation:

- **Vertex** → point in 3D
- **Edge** → curve segment between vertices
- **Face** → bounded surface region
- **Shell** → connected set of faces
- **Solid** → closed shell with volume

Surfaces: Plane, Cylinder, Cone, Sphere, Torus, NURBS

### Exact Predicates

Shewchuk's adaptive-precision predicates via `robust` crate for robust geometric decisions:
- `orient2d`, `orient3d` — orientation tests
- `incircle`, `insphere` — containment tests
- Used in boolean face classification, trimming, mesh point-in-solid

### Boolean Pipeline (4-stage)

1. **AABB Filter** — broadphase candidate detection
2. **Surface-Surface Intersection** — analytic + sampled fallback
3. **Face Classification** — ray casting + winding number
4. **Sewing** — trim, split, merge with topology repair

### Constraint Solver

Levenberg-Marquardt with adaptive damping. Constraints: Coincident, Horizontal, Vertical, Parallel, Perpendicular, Tangent, Distance, Length, Radius, Angle, Equal Length, Fixed.

### Direct BRep Ray Tracing

Pixel-perfect rendering without tessellation via `vcad-kernel-raytrace`:
- Analytic ray-surface intersection for all surface types
- WebGPU compute shader pipeline
- BVH acceleration with SAH construction
- Trimmed surface handling
- App toggle between standard (tessellated) and ray-traced modes

### Physics Simulation

Rapier3D-based physics via `vcad-kernel-physics`:
- BRep-to-physics conversion (rigid bodies, collision shapes)
- Joint support: Revolute, Prismatic, Cylindrical, Ball, Fixed
- Gym-style RL interface: `reset()`, `step(action)`, `observe()`
- Three action types: torque, position targets, velocity targets
- MCP tools for AI agent training

### Web App

- **Viewport:** React Three Fiber with custom shaders, ray-traced mode
- **State:** Zustand stores (document, selection, UI)
- **Feature tree:** Hierarchical part/instance/joint view
- **Property panel:** Scrub inputs for parameters
- **Sketch mode:** 2D constraint UI
- **Assembly mode:** Instances, joints, forward kinematics
- **Drawing mode:** Orthographic projections, dimensions

### Document Format

`.vcad` files are JSON containing:
- Parametric DAG (operations reference parents)
- Part definitions and instances
- Joints with kinematic state
- Material assignments
- Sketches with constraints

## App Features

| Feature | Status |
|---------|--------|
| Primitives (box, cylinder, sphere, cone) | ✅ |
| Boolean operations | ✅ |
| Transforms (translate, rotate, scale, mirror) | ✅ |
| Patterns (linear, circular) | ✅ |
| Fillets and chamfers | ✅ |
| Sketch mode with constraints | ✅ |
| Extrude, Revolve, Sweep, Loft | ✅ |
| Shell operation | ✅ |
| Assembly with joints | ✅ |
| Forward kinematics | ✅ |
| Physics simulation (Rapier3D) | ✅ |
| 2D drafting views | ✅ |
| DXF export | ✅ |
| STEP import (drag-drop, file picker) | ✅ |
| STL/GLB export | ✅ |
| Direct BRep ray tracing | ✅ |
| Undo/redo | ✅ |

## Headless Interfaces

**Rust CLI:**
```bash
vcad export input.vcad output.stl   # Export to STL/GLB/STEP
vcad import-step input.step out.vcad
vcad info input.vcad                # Show document info
```

**MCP Server** (for AI agents):
- `create_cad_document` — create parts from primitives + operations
- `export_cad` — export to STL or GLB
- `inspect_cad` — get volume, area, bbox, center of mass
- `create_robot_env` — create physics simulation from assembly
- `gym_step` — step simulation with torque/position/velocity actions
- `gym_reset` — reset simulation to initial state
- `gym_observe` — get current observation without stepping
- `gym_close` — clean up simulation environment

## Conventions

- **Coordinate system: Z-up** — X right, Y forward, Z up (standard CAD convention)
  - Cube `(sx, sy, sz)` → corner at origin, extends to `(sx, sy, sz)` — `sz` is height
  - Cylinder axis is along **Z** — already vertical, no rotation needed
  - Grid lies in the XY plane; Z is the vertical axis
  - Assembly instance transforms and joint anchors use this Z-up frame
  - The Three.js renderer wraps kernel geometry in a `-90°` X rotation to convert Z-up → Y-up for display
- `#![warn(missing_docs)]` on public items
- Tests in `#[cfg(test)] mod tests` at file bottom
- Units are `f64`, conventionally millimeters
- IR types use `#[serde(tag = "type")]` for JSON discrimination
- App components in `packages/app/src/components/`
- Stores in `packages/app/src/stores/`

## Adding Functionality

**New kernel feature:**
1. Add to appropriate `vcad-kernel-*` crate
2. Expose via `vcad-kernel` unified API
3. Add WASM bindings in `vcad-kernel-wasm`
4. Run `cargo test --workspace && cargo clippy --workspace -- -D warnings`

**New app feature:**
1. Add store logic in `packages/app/src/stores/`
2. Add UI components in `packages/app/src/components/`
3. Wire up in `App.tsx`
4. Run `npm run build -w @vcad/app`

**New IR operation:**
1. Add variant to `CsgOp` in `crates/vcad-ir/src/lib.rs`
2. Mirror in `packages/ir/src/index.ts`
3. Add evaluation logic in `packages/engine/src/evaluate.ts`

## Changelog

The changelog lives in `changelog/entries/` — **one JSON file per entry**. The
rolled-up `/CHANGELOG.json` is generated by `scripts/build-changelog.mjs`
(runs as `postinstall` and as part of `@vcad/core`'s build) and is gitignored,
so concurrent PRs never collide on it.

Update the changelog when:
- Adding user-facing features (category: `feat`)
- Fixing user-facing bugs (category: `fix`)
- Making breaking changes (category: `breaking`)
- Significant performance improvements (category: `perf`)

Skip changelog for:
- Internal refactors
- Test-only changes
- Documentation updates (unless significant)
- Dependency bumps

To add an entry, drop a new file at `changelog/entries/<id>.json`:

```json
{
  "id": "YYYY-MM-DD-short-slug",
  "version": "current version from package.json",
  "date": "YYYY-MM-DD",
  "category": "feat|fix|breaking|perf|docs",
  "title": "Short title (max 60 chars)",
  "summary": "One sentence description (max 200 chars)",
  "features": ["relevant", "tags"],
  "mcpTools": ["if", "applicable"]
}
```

The filename must match the `id` field. Order is derived from `date` and `id`
(newest first), so placement isn't a concern. The schema at
`/changelog.schema.json` validates the entry shape; run
`npm run changelog:build` to regenerate the rolled-up file on demand.
