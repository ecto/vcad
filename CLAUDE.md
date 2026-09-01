# CLAUDE.md

Instructions for AI agents working on vcad.

## Overview

vcad is an open-source parametric CAD system aiming to replace Fusion 360, Onshape, and similar tools. It features a custom BRep kernel written in Rust, a React/Three.js web app, and AI-native interfaces via MCP.

**Live app:** https://vcad.io

## Prerequisites

vcad depends on the `tang` math workspace and the `phyz` physics workspace at
**sibling paths** (`../tang`, `../phyz`). Clone both next to vcad before running
`cargo build` — a default build fails without either (`vcad-kernel-physics` and
`vcad-sim` are in default-members, and `vcad-kernel-wasm`'s default `physics`
feature pulls phyz in too):

```bash
git clone git@github.com:ecto/tang.git ../tang
git clone git@github.com:ecto/phyz.git ../phyz
```

Cargo paths in the workspace (`tang`, `tang-la`, `tang-expr`) all point at
`../tang/crates/*`; `vcad-kernel-physics` and `vcad-sim` point at
`../../../phyz/crates/*` (i.e. `../phyz` relative to the repo root).

**Fresh worktrees** (`.claude/worktrees/*`) start with no `node_modules` — run `npm ci`
before any npm/tauri command. Tauri needs the `cargo-tauri` binary (installed globally
via `cargo install tauri-cli`); the npm scripts invoke it as `cargo tauri`, so no local
`tauri` on PATH is required.

Worktree roots live at `.claude/worktrees/<name>`, so the sibling path deps
resolve to `.claude/worktrees/tang` and `.claude/worktrees/phyz`. Symlinks
inside `.claude/worktrees/` make this work (`tang -> /Users/cam/Developer/tang`,
`phyz -> /Users/cam/Developer/phyz`); they must exist — or be created — before
`cargo` commands will build from a worktree (run from the **main** checkout):

```bash
ln -sfn "$(cd .. && pwd)/tang" .claude/worktrees/tang
ln -sfn "$(cd .. && pwd)/phyz" .claude/worktrees/phyz
```

After `npm ci`, the app imports from `@vcad/core`, `@vcad/engine`, `@vcad/ir`, `@vcad/mcp`
which all resolve to `dist/index.js` — so workspace packages must be built before
`npm run dev` or `tauri:dev` will start. Fastest path:

```bash
VCAD_WASM_SKIP=1 npm run build --workspaces --if-present
```

`VCAD_WASM_SKIP=1` skips the wasm-pack rebuild when `packages/kernel-wasm/vcad_kernel_wasm*`
artifacts are already checked in; drop it if you need a fresh kernel WASM.

**Never commit the generated `packages/kernel-wasm/vcad_kernel_wasm*` artifacts from a
feature branch** — wasm-pack output is not byte-reproducible, so two branches that each
rebuilt them merge-conflict on every merge even when their Rust changes don't overlap.
They have a single writer: `.github/workflows/wasm-refresh.yml` rebuilds and commits them
on `main` after any kernel-source merge, and CI (`wasm-artifact-guard`) fails a PR that
touches them. If you accidentally committed a rebuild, drop it with
`git checkout origin/main -- 'packages/kernel-wasm/vcad_kernel_wasm*'`. PR CI does not
depend on the checked-in copies — the TypeScript job consumes WASM built from source.

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

## MCP server distribution — never point a config at dist/index.js

A config referencing a checkout's `packages/mcp/dist/index.js` can silently
serve stale code (the 2026-07-23 ice-viz session lost hours to a dist built 4
days earlier on a parked feature branch — missing the parts DB, fix_drc,
crystal footprints, and inline previews, and minting dead artifact URLs).
Two supported channels instead:

**User mode (default) — published npm package:**

```json
{ "command": "npx", "args": ["-y", "@vcad/mcp"] }
```

`.github/workflows/mcp-publish.yml` publishes a self-contained bundle (server
+ kernel WASM in one tarball, version/sha/time baked in — see
`packages/mcp/scripts/build-npm.mjs`, which mirrors `services/mcp/build.sh`)
on every main merge touching `packages/**` or `lib/**`, versioned
`<base>-main.<run>` on the `latest` dist-tag. The kernel structurally cannot
lag the server: they ship in the same immutable tarball, smoke-tested
(`scripts/mcp-npm-smoke.mjs` — boot + WASM init + initialize handshake)
before publish. Needs the `NPM_TOKEN` repo secret.

**Contributor mode — self-validating launcher, for sessions hacking on the
server itself (runs the branch under your feet):**

```json
{ "command": "node", "args": ["/path/to/vcad/packages/mcp/scripts/serve.mjs"] }
```

`serve.mjs` fingerprints the checkout (git tree hash of `packages/` + `lib/`
plus a digest of uncommitted changes), rebuilds the workspace when the stamp
in `dist/.build-stamp.json` doesn't match, warns on stderr when the checkout
is behind `origin/main`, then execs the server. A raw `npm run build` doesn't
write the stamp, so the next `serve` triggers one redundant rebuild and then
stamps — self-correcting, not an error.

## Preview-shaped complaint? Check `server_info` FIRST

"Preview unavailable", a widget that stopped updating, an `Unknown
document_id` — before debugging the renderer, call `server_info` and read
`uptime_s` and `instance_id`. A low `uptime_s` (or an `instance_id` that
changed since the last call) means **the server restarted**, not that the
feature broke. This is the single fastest way to separate "the server
restarted" from "the feature is broken", and it costs one call.

Why it looks like a renderer bug: on a restart every live `document_id` dies
at the same instant, so every mounted widget in the transcript — including
ones that rendered fine minutes earlier — goes dark simultaneously. The
inline `_meta` GLB can't rescue them (it only covers first paint on mount
tools, capped at 1.5M base64 chars), so a dead session's widget never
recovers. The viewer now says *"session lost (server restarted) — re-run the
last authoring call"* instead of the old bare "preview unavailable".

Session ids carry the minting process's boot token, so the server tells the
two cases apart: an id from a dead process reports `SESSION LOST TO A SERVER
RESTART`, a genuinely unknown id reports a typo. Every tool result also
stamps `_meta['io.vcad/build']` with `boot_token` + `instance_id` +
`session_durable`, so a client can detect a restart on the very next call
without polling.

**Durability.** Local runs persist sessions to `~/.vcad/mcp-sessions`
(override with `VCAD_MCP_SESSION_DIR`, opt out with
`VCAD_MCP_DISK_SESSIONS=0`), so a restart no longer destroys a long build.
Hosted deploys need `SUPABASE_URL` + `SUPABASE_SERVICE_ROLE_KEY`; a
serverless instance never uses the disk store (its filesystem is ephemeral
and unshared, so it would report durability it can't provide). When
`server_info` reports `durable:false`, session-minting tools say so in their
result — keep the authoring source rather than treating the server as
storage.

**Artifacts are separate and shorter-lived.** `artifact_store:"in-memory"`
means `render_view` PNG URLs die on the same restart. Treat any artifact link
handed to a user as short-lived unless the artifact store is durable.

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
├── crates/                        # Rust workspace (~285K LOC)
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
│   ├── vcad-kernel-physics/       # phyz physics simulation
│   ├── vcad-kernel-urdf/          # URDF robot description import
│   ├── vcad-kernel-assembly/      # Posed assemblies: mates (coaxial, planar-offset,
│   │                              # pattern-phase), interference, exploded views
│   ├── vcad-kernel-cam/           # 2.5D CAM toolpath generation + G-code post
│   ├── vcad-kernel-stocksim/      # CAM stock sim (octree SDF) + toolpath verification oracle
│   ├── vcad-kernel-topopt/        # SIMP topology optimization (voxel FEA + surface nets)
│   ├── vcad-kernel/               # Unified kernel API
│   ├── vcad-kernel-wasm/          # WASM bindings for browser
│   ├── vcad-ir/                   # Intermediate representation
│   ├── vcad-receipt/              # Unified fail-closed verification receipt schema
│   ├── vcad-cli/                  # CLI tool
│   ├── vcad-render/               # Standalone .vcad → isometric SVG renderer
│   └── vcad/                      # Legacy CSG library (manifold-based)
│
│   # Analysis/solver crates beyond the app's modeling surface (consumer wiring
│   # noted per entry; most are self-contained pending vcad-receipt/MCP
│   # registration — each ships its own fail-closed predicted-basis claim
│   # family and a docs/<domain>-m0.md milestone ladder):
│   # - vcad-kernel-diff         # Differentiable seam, COMPLETE (M0–M11 + closeout):
│   #                            # dx/dθ of frozen tessellations, mass-property QoIs,
│   #                            # differentiable fillet radius, reverse-mode adjoint,
│   #                            # seeding synthesis, cone/torus coverage incl. second
│   #                            # order, L-BFGS optimizer
│   #                            # (docs/differentiable-seam-m0-m2.md … -m11.md).
│   #                            # Consumers: vcad-kernel-physics::diff (rollout
│   #                            # gradients — mass-property core, anchor channel,
│   #                            # surface skin; M11 adds exact ∂J/∂p via the
│   #                            # phyz::diff trajectory adjoint and
│   #                            # contact_rollout_gradient — contact dynamics
│   #                            # priced through the seam) and vcad-eval::diff
│   #                            # (document_parameter_gradient — d(mass props)/d(named
│   #                            # .vcad parameter))
│   # - vcad-kernel-particle     # Charged-particle optics (fusors, shielded-
│   #                            # grid IEC), M0–M6 COMPLETE + wired: axisym
│   #                            # Poisson (SOR) + exact ring-coil B (elliptic)
│   #                            # + Boris tracing + Bosch-Hale D-D yield +
│   #                            # charge exchange + discrete adjoint (FD-
│   #                            # validated 0.1–0.8%) + DeviceSpec seam.
│   #                            # Reproduces the shielded-grid effect
│   #                            # (arXiv:1510.01788), r_L ∝ √V. Claims ride
│   #                            # vcad-receipt's open domain vocabulary
│   #                            # (vcad.particle-claims/1: Q, distance-to-
│   #                            # Lawson; predicted → Provisional, never
│   #                            # Pass) and the MCP tools
│   #                            # simulate_charged_particles /
│   #                            # optimize_electrodes (multi-start FD search;
│   #                            # adjoint optimizer in-crate) are live.
│   #                            # (docs/particle-optics-m0.md, -paper-draft,
│   #                            # shielded-grid-experiment.md)
│   # - vcad-kernel-tolerance    # Tolerance stackups: WC/RSS/seeded-MC +
│   #                            # exact closed-form sensitivities, GD&T
│   #                            # gauged fits, min-cost allocation,
│   #                            # measurement binding; vcad.tolerance-claims/1
│   #                            # (docs/tolerance-m0.md)
│   # - vcad-kernel-thermal      # Voxel FV heat conduction, steady + transient
│   #                            # (PCG, harmonic-mean faces) + one-extra-solve
│   #                            # adjoint; vcad.thermal-claims/1
│   #                            # (docs/thermal-m0.md … thermal-m5-m6.md)
│   # - vcad-kernel-photonics    # 2D FDTD (Yee, CPML) + discrete adjoint +
│   #                            # density topology optimization → fab-ready
│   #                            # GDS; vcad.photonics-claims/1
│   #                            # (docs/photonics-m0.md, photonics-tapeout.md)
│   # - vcad-kernel-em           # 2D FV magneto/electrostatics (axisym ψ,
│   #                            # planar A_z, electro φ), two-route QoIs with
│   #                            # cross_route_residual, nonlinear B–H, AC
│   #                            # eddy, discrete adjoint; vcad.em-claims/1
│   #                            # (docs/em-m0.md, em-measurement-pack.md)
│   # - vcad-kernel-antenna      # Thin-wire MoM (EFIE, Galerkin): Z_in/S11/
│   #                            # gain, NEC-2-benchmarked, fail-closed thin-
│   #                            # wire gates, adjoint dZ_in/dp, NanoVNA
│   #                            # measurement pack; vcad.antenna-claims/1
│   #                            # (docs/antenna-m0.md, -measurement-pack.md)
│   # - vcad-kernel-neutronics   # 5-group analog MC neutron transport (exact
│   #                            # elastic kinematics) + H*(10) dose + adjoint-
│   #                            # diffusion thickness gradients; fission
│   #                            # refused permanently; vcad.neutronics-claims/1
│   #                            # (docs/neutronics-m0.md)
│   # - vcad-kernel-acoustics    # Air-side acoustics, M0 COMPLETE: axisym
│   #                            # (r,z) Helmholtz field solve (vertex-centred
│   #                            # finite volume — conservative, symmetric →
│   #                            # reciprocal to 4.5e-16; direct block-Thomas
│   #                            # since the operator is indefinite, SOR would
│   #                            # diverge) + lumped duct-mass/cavity-
│   #                            # compliance/Helmholtz-tuning oracles +
│   #                            # baffled-piston radiation (Rayleigh + closed
│   #                            # form) + port-sizing optimizer. Reproduces
│   #                            # cylinder axial modes fₙ=n·c/2L (0.04-0.1%),
│   #                            # 2nd-order convergence to a 0.005% floor.
│   #                            # Complements the *structural* simulate_strike
│   #                            # (TS beam FEM): seam is surface-velocity-in,
│   #                            # pressure-out — coupling is M2. Lossless (Q
│   #                            # optimistic), pressure-release mouth reads
│   #                            # tuning ~15% high (M1 radiation mouth closes
│   #                            # it). vcad.acoustics-claims/1 (predicted →
│   #                            # Provisional; mic + swept sine close it, the
│   #                            # glockenspiel precedent). Flagship
│   #                            # examples/ported_box.rs. (docs/acoustics-m0.md)
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
├── lib/                           # Stdlib for vcad: loon CAD library + DFM rule packs
│   ├── src/lib.loon               # bundled into crates/vcad-loon via include_str!
│   └── dfm/                       # DFM rule packs (.toml) bundled into vcad-kernel-dfm
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

phyz-based articulated physics via `vcad-kernel-physics`:
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
| Physics simulation (phyz) | ✅ |
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

# Routed board → complete fab package + DRC-delta receipt, in one command.
# Calibration is opt-in and logged; the loop fails closed (exit 1, no
# fabrication files) if route-attributable violations don't reach zero.
vcad fab-prep routed.pcb.json -o out/ --calibrate-rules
```

**Static SVG renderer:** [`vcad-render`](crates/vcad-render) projects a `.vcad` to a drafting-style isometric SVG. Used by the mecheval leaderboard, but standalone — handy for docs, marketing, and README diagrams.
```bash
cargo build -p vcad-render
target/debug/vcad-render path/to/part.vcad > out.svg
```

**MCP Server** (for AI agents):
- `create_cad_document` — create parts from primitives + operations
- `export_cad` — export to STL or GLB
- `inspect_cad` — get volume, area, bbox, center of mass
- `check_clearance` — min distance / penetration depth between part groups;
  labeled assertions persist on the document and re-verify via
  `build_receipt` / `verify_receipt` as Holds/Stale/Violated
- `render_view` — render the session document to an isometric PNG (agent eyes)
- `flat_pattern_from_solid` — flat pattern (DXF + bend table) for a part
  modelled as an ordinary solid; batches a document into unique patterns ×
  quantity and fails closed on a volume-mismatched (non-sheet) part
- `topology_optimize` — SIMP topology optimization: stiffest material layout
  for given loads/supports inside a box envelope or an existing part's volume;
  result lands in the document as a frozen mesh part
- `fab_prep` — routed board → fab-ready, in one call: opt-in (logged) rule
  calibration, verdict ladder, strip-and-re-route fix loop, dangling-copper
  prune. Returns a DRC-delta receipt reporting route-attributable violations
  against the SAME board stripped of all routing — absolute zero is not
  achievable on an imported fixture, so both numbers are always given. Fails
  closed; `export_gerber`'s clean-DRC gate still stands
- `verify_part` / `list_eval_tasks` — grade the document against mecheval
  benchmark tasks via the official `mecheval-grade` binary (self-grading
  oracle; the benchmark harness excludes these during scored runs)
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
5. **Update the kernel features catalog** at
   `packages/docs/content/architecture/kernel-features.mdx` — every kernel
   capability is listed there. Add (or revise) the section for your feature,
   and if it's a modeling operation, illustrate it with a render generated by
   the kernel itself:
   - Write the example in loon and render it directly — `vcad-render` (and the
     `vcad` CLI's `export`/`info`) take `.loon` source anywhere a `.vcad` goes,
     evaluating it on the way in with `[use ...]` imports resolved against the
     file's own directory:
     `cargo run -p vcad-render -- part.loon --jpeg out.jpg --size 720 --quality 88`
   - Need the IR itself (diffing, piping, CI)? `cargo run -p vcad-loon --example
     loon2vcad -- part.loon > part.vcad`
   - `--size` also takes `WxH`; for a part with a strong aspect ratio, add
     `--auto-aspect` (canvas follows the projection) or, with PNG output,
     `--trim [--trim-margin <px>]` (crop to the drawn content) so the subject
     isn't a thin ribbon in a mostly empty square
   - API gotcha: `vcad_loon::eval_vcad` returns a serde-serializable
     `Document`; `eval_vcad_to_value` returns a loon `Value`, which is **not**
     `Serialize` (`serde_json::to_string(&value)` fails to compile, E0277)
   - Save to `packages/docs/public/assets/kernel/<feature>.jpg` and embed the
     loon source next to the image in the MDX (the page doubles as a cookbook)
   - Loon gotcha: `[arc x1 y1 x2 y2 cx cy ccw]` takes a boolean `ccw`
     (`true`/`false`, not `0`/`1`)
   Crate-count or capability claims on that page (e.g. "~25 crates") should be
   kept in sync when crates are added or removed.

**New app feature:**
1. Add store logic in `packages/app/src/stores/`
2. Add UI components in `packages/app/src/components/`
3. Wire up in `App.tsx`
4. Run `npm run build -w @vcad/app`

**New IR operation:**
1. Add the variant to `CsgOp` in `crates/vcad-ir/src/lib.rs`, carrying the same
   `#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]` / `ts(...)` attrs the
   sibling variants use (Rust is the single source of truth for IR types).
2. Run `npm run ir:gen` to regenerate `packages/ir/src/generated.ts` from Rust
   — the TS types are **generated, not hand-mirrored**. `npm run ir:check`
   fails CI when the committed file drifts from the Rust definitions. Add an
   `Extract<CsgOp, { type: "…" }>` alias in `index.ts` only if a consumer needs
   the variant by name.
3. Add evaluation logic in `packages/engine/src/evaluate.ts`

`ir:gen` bundles ts-rs bindings from **two** crates: `vcad-ir` and
`vcad-receipt` (the unified receipt schema). Type names must be unique across
both — the script fails loudly on collisions.

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
- Build, CI, and deployment-infra fixes (e.g. esbuild/bundler config,
  Vercel/Fly wiring, hosted-server outages) — the changelog tracks changes to
  what the product *does*, not how it's built or shipped. A fix only belongs
  here if a user would notice a behavior change, not merely that a service is
  reachable again.

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
