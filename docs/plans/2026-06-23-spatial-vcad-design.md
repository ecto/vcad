# Spatial vcad — Native Apple App Design

**Date:** 2026-06-23
**Status:** Design approved; implementing M0–M2 (Mac-first)
**Author:** Cam + Claude

## Goal

A native Apple implementation of vcad built to win an Apple Design Award. Not
"Fusion-but-native" — a sharp, single point of view that could only exist on
Apple's stack: **a parametric BRep kernel you can reach into and edit with your
hands.** The kernel re-solves live as you scrub a parameter, at 90fps, in front
of you.

The web app (vcad.io) remains; this is a net-new native flagship, not a port of
the React UI.

## Decisions (locked)

| Decision | Choice | Why |
|---|---|---|
| Platform | Full ecosystem — Mac + iPad + visionOS | visionOS carries the award flag; Mac/iPad ride the same core |
| Product role | Halo flagship spike | One jaw-dropping vertical slice, not web-app parity |
| Hero interaction | Parametric scrub | vcad's thesis; reuses the general re-eval path; zero new kernel ops |
| visionOS render | RealityKit + `LowLevelMesh` (shared space) | Native polish + real-world lighting for free; re-solve latency is the real problem and it's our home turf |
| Mac/iPad viewport | `RealityView` everywhere | One scene graph, three input models → ~85–90% Swift code-share |
| Mac app personality | One obsessive window | Calm, focused; one surface to perfect → easiest to make award-grade |
| Source of truth | Rust-authoritative | Kernel owns the DAG + geometry; Swift holds an opaque handle + a thin `@Observable` projection |

## Architecture — the layer cake

```
┌─────────────────────────────────────────────────────────────┐
│ App targets (per-platform, thin)                            │
│   vcad-mac · vcad-ipad · vcad-vision                        │
│   — window/scene/menus + input adaptation only             │
├─────────────────────────────────────────────────────────────┤
│ VcadUI (Swift, shared ~90%)                                 │
│   feature tree · inspector · scrub fields · ⌘K palette     │
├─────────────────────────────────────────────────────────────┤
│ VcadScene (Swift, shared ~95%)                              │
│   RealityKit entities · LowLevelMesh streaming ·           │
│   handle affordances · BRep-raycast picking                │
├─────────────────────────────────────────────────────────────┤
│ VcadModel (Swift, shared 100%)                              │
│   @Observable doc projection · UndoManager · sync          │
├─────────────────────────────────────────────────────────────┤
│ VcadKernelFFI (Swift wrapper, shared 100%)                  │
│   thin Swift over the C ABI                                 │
├─────────────────────────────────────────────────────────────┤
│ vcad-ffi (Rust, NEW — the only new Rust)                    │
│   C-ABI bridge · cbindgen header · catch_unwind boundary   │
├─────────────────────────────────────────────────────────────┤
│ vcad-kernel + vcad-kernel-raytrace (Rust, UNCHANGED)        │
│   BRep · booleans · fillet · tessellate · CPU raycast      │
└─────────────────────────────────────────────────────────────┘
```

**`vcad-ffi` — the only new Rust, ~one file, four jobs:**

- `vcad_open(json) -> Handle` — load a `.vcad` doc; kernel owns the authoritative DAG.
- `vcad_handles(Handle) -> [Param{id, label, value, anchor_xyz}]` — editable params with 3D anchor positions (where handles float).
- `vcad_set_param(Handle, id, value, buffer_ptr)` — the hot path: re-eval the DAG subtree downstream of the param, re-tessellate the changed part, **write vertices/indices straight into a Swift-owned buffer** (zero-copy into `LowLevelMesh`).
- `vcad_raycast(Handle, ray) -> Feature{kind, id}` — the BRep raytracer, repurposed to pick which edge/face a click/pinch landed on.

`catch_unwind` at the boundary so a Rust panic never crosses into Swift. Opaque
handles, explicit free.

## The hot loop (the whole demo lives here)

```
pinch/drag-scrub delta
  → vcad_set_param(handle, "fillet_r", 6.2, lowLevelMeshBuffer)
  → Rust re-solves only the downstream subtree, writes verts into the buffer
  → Swift commits the LowLevelMesh update
  → RealityKit renders next frame
target round-trip: < 11ms (90fps)
```

### Latency strategy (the actual hard problem)

Re-solving an arbitrary boolean+fillet DAG every frame is **not** guaranteed.
Mitigations, in order:

1. **Incremental eval** — only re-run DAG nodes downstream of the changed param; memoize unchanged subtrees.
2. **Proxy-during-drag** — coarse tessellation segments while scrubbing, exact solve on release.
3. **Background actor + triple-buffered `LowLevelMesh`** — kernel solves off the render thread; frame never blocks.
4. **Scope the hero part to the budget** — a bracket that re-solves in <11ms, not a 5000-feature engine block. Pick the demo part to fit the frame budget; don't fake it.

## World-class Mac shell

SwiftUI-first (macOS 26 / Tahoe), AppKit only where it bleeds.

- **`DocumentGroup` + `ReferenceFileDocument`** for `.vcad` — free autosave, edited-dot, draggable proxy icon, document versions, iCloud, duplicate, revert. The Tauri app hand-rolls weak versions of these.
- **Window anatomy:** `NavigationSplitView` — feature tree (leading) │ `RealityView` viewport (content) │ `.inspector()` property panel (trailing). All SwiftUI-native; all adopt Liquid Glass automatically.
- **Full menu bar** via `CommandGroup`s — every op keyboard-bound.
- **Drop to AppKit** only twice: the (eventual) viewport host if `RealityView` is insufficient, and the feature tree if a huge DAG outpaces SwiftUI `List` (NSOutlineView).

### The craft layer (what judges feel in 10s)

- **Liquid Glass chrome floating over live 3D** — the defining macOS-26 look.
- **Trackpad haptics on every scrub** (`NSHapticFeedbackManager`) — detent clicks on numeric fields, haptic on snap. Wildly underused by competitors.
- **Command palette (⌘K)** — fuzzy-jump to any op/parameter.
- **Quick Look + Spotlight + Thumbnail extensions** for `.vcad`.
- **App Intents / Shortcuts** — scriptable; the hook for Apple Intelligence later.
- **120Hz ProMotion camera** with momentum, instant launch, low idle memory.
- **Continuity** — Handoff Mac→Vision Pro→iPad, Universal Control, Sidecar.

## Code-share map (the payoff of the locked decisions)

| Layer | Shared |
|---|---|
| Rust kernel + `vcad-ffi` | 100% |
| `VcadModel` (doc projection + sync) | 100% |
| `VcadScene` (entities, LowLevelMesh, picking, handles) | ~95% |
| `VcadUI` (inspector, tree, scrub fields, ⌘K) | ~90% |
| Input + window chrome + menus | per-platform (thin) |

~85–90% of the Swift is written once.

## Sequencing — Mac is the on-ramp to the flag

Build on Mac first (debuggable, keyboard, Instruments), then **lift the proven
`VcadScene` to visionOS**. We walk the riskiest mile — the hot loop — where we
can actually profile it.

- **M0 — First light:** `vcad-ffi` → static lib + C header, linked into a SwiftPM Mac app; render one kernel mesh in a `RealityView`. Proves Rust→Swift→RealityKit end-to-end.
- **M1 — The Mac shell:** `DocumentGroup` open/save `.vcad`, three-pane window, trackpad camera, raycast selection, inspector with haptic scrub fields. A usable native Mac editor.
- **M2 — The hot loop, on Mac:** param handles → `vcad_set_param` → `LowLevelMesh` stream → live fillet-radius re-solve. Profile; add proxy-during-drag. **The riskiest mile, walked where we can see.**
- **M3 — Lift to visionOS:** same `VcadScene`, swap input to hand tracking, volumetric window, grounding shadows + IBL + spatial-audio detents. The 90-second reel. *(Requires Xcode.)*
- **M4 — iPad + Continuity + polish:** Pencil Pro, Handoff, Quick Look/Spotlight, ⌘K, technical-edge material.

## Environment (this Mac, verified 2026-06-23)

- Apple Silicon (arm64), macOS 26.5.1, Swift 6.3.2, Rust 1.95.
- **Full Xcode is NOT installed** — Command Line Tools only (`xcodebuild` unavailable).
- **The CLT SDK ships RealityKit, SwiftUI, RealityFoundation, CompositorServices, MetalKit** — frameworks resolve and link. **M0–M2 (Mac) target the CLT toolchain (swiftc + SwiftPM), no Xcode required.** *(Being verified empirically in M0.)*
- visionOS (M3) **does** require Xcode (no visionOS SDK in CLT) — install deferred until M3.
- `vcad-kernel` is lean (no wgpu/GPU/raytrace deps) → clean CPU-only static lib.
- `tang` resolves via symlink from the worktree → Rust workspace builds here.

## Risks

| Risk | Mitigation |
|---|---|
| Re-solve latency for non-trivial parts | Incremental eval + proxy-during-drag + scope hero part |
| `RealityView` lacks crisp CAD edges (PBR engine) | v1 renders edges as thin tube entities from BRep edge curves; `ShaderGraphMaterial` technical pass in M4 |
| FFI memory/panic safety across boundary | Opaque handles, explicit free, `catch_unwind` at every boundary fn |
| Face/Edge id stability across re-solve (handle anchoring) | TBD by grounding workflow; fall back to anchoring by geometric proximity if ids churn |
| Rust IR→mesh evaluator might be TS-only | TBD by grounding workflow; if so, evaluate via the kernel's primitive/op API directly from the FFI |
| CLT cannot bundle/run a windowed app | Wrap SwiftPM binary in a hand-built `.app` + ad-hoc codesign; verified in M0 |

## Grounding appendix

_(Filled in from the `vcad-native-grounding` workflow: exact kernel API surface,
Rust eval path, parameter model, raycast/id stability, FFI recipe, RealityKit
specifics. See workflow run `wf_ea10e1f0-fb2`.)_
