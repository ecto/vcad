# Issue Drafts — Legendary-Tier CAD Gap Audit

Generated from a full-codebase audit (Rust kernel + TS/app) against a
"legendary-tier" capability list for parametric CAD. Each draft below is
ready to be filed as a GitHub issue. Backlog items are recommended for
near-term implementation; ideas are parked for future discussion.

---

## BACKLOG (recommended for near-term work)

### 1. Boolean kernel — degenerate-input test suite + repair fixes

**Labels:** `backlog`, `kernel`, `robustness`

**Background.** The 4-stage boolean pipeline in `crates/vcad-kernel-booleans/`
(~9.9K LOC) already has explicit degenerate-handling hooks —
`collapse_degenerate_half_edges()` in `repair.rs:313`, zero-area face
detection in `split.rs`, sphere-cap pole-loop special cases in `trim.rs` —
but there is **no test coverage** for the hard cases, and arc-split
geometry is marked TODO at `lib.rs:731` and `lib.rs:983`. Booleans on
degenerate input are the #1 thing every competing kernel ships broken;
fixing ours would be a concrete differentiator.

**Deliverables.**
- `crates/vcad-kernel-booleans/tests/degenerate_cases.rs` with golden
  test fixtures for each of:
  1. Coincident co-planar faces between operands
  2. Exact tangent surface–surface intersections (cyl tangent to plane)
  3. Knife-edge (face–face single-line) intersections
  4. Self-intersecting input recovery
  5. Floating-point near-miss (within epsilon of coincident)
  6. Arc-split geometry (resolve the existing TODOs)
- Fixes to `repair.rs` / `split.rs` / `trim.rs` as needed to make each
  test pass.
- Resolve `lib.rs:731` and `lib.rs:983` arc-split TODOs.

**Success criteria.** `cargo test -p vcad-kernel-booleans` passes with
the new tests. Each of the 6 categories has at least one passing fixture.

---

### 2. BRep feature recognition (holes, pockets, slots, fillets, chamfers)

**Labels:** `backlog`, `kernel`, `ai-native`, `mcp`

**Background.** No feature-recognition pass exists today. Adding it
unlocks four differentiators at once: MCP semantic search ("find all M6
through-holes"), direct-modeling intent inference, MBD/PMI auto-annotation,
and AI workflow correctness checks. The topology is already in half-edge
form, so this is mostly a classification pass over existing data.

**Deliverables.**
- New crate `crates/vcad-kernel-recognition/`:
  - `detect_holes(solid) -> Vec<HoleFeature>` — cylindrical face + ring
    of planar faces + concavity test; classifies through / blind /
    countersink / counterbore via axial ray test.
  - `detect_pockets(solid) -> Vec<PocketFeature>` — bounded planar bottom
    + surrounding walls.
  - `detect_fillets(solid) -> Vec<FilletFeature>` — tangent-continuous
    cylindrical/toroidal face between two adjacent faces.
  - `detect_chamfers(solid) -> Vec<ChamferFeature>` — narrow planar face
    bridging two other planar faces at a non-90° angle.
- Expose via `vcad-kernel` unified API and `vcad-kernel-wasm`.
- MCP tools in `packages/mcp/`:
  - `recognize_features(document, part_id) -> FeatureReport`
  - `find_features(document, query)` — e.g. `{type: "hole", diameter: 6.0}`
- App: "Detected features" panel in property sidebar.

**Success criteria.** Golden fixtures (bracket, enclosure, flanged hub)
report correct feature counts. MCP tool works end-to-end from Claude.

---

### 3. STEP AP214 — assemblies, trimmed NURBS, PMI

**Labels:** `backlog`, `kernel`, `step`

**Background.** `crates/vcad-kernel-step/` (~3.4K LOC) reads
`MANIFOLD_SOLID_BREP` + planar/cylindrical/spherical/toroidal/B-spline
surfaces — but only as single-body parts with untrimmed faces.
Industry STEP files break immediately because they rely on
`PRODUCT_DEFINITION` assembly hierarchy and `TRIMMED_CURVE` on NURBS
surfaces. This blocks every realistic supplier-part import use case.

**Deliverables.**
1. **Assembly import.** `PRODUCT_DEFINITION`,
   `NEXT_ASSEMBLY_USAGE_OCCURRENCE`, `SHAPE_REPRESENTATION_RELATIONSHIP`
   → `vcad-ir` Part + Instance hierarchy with local transforms.
2. **Trimmed NURBS.** `TRIMMED_CURVE` / `BOUNDED_SURFACE` support in
   `vcad-kernel-nurbs` plus STEP reader wiring. Trim loop evaluation in
   the ray tracer and tessellator.
3. **PMI pass-through (stretch).** `DIMENSIONAL_*`, `TOLERANCE_*`,
   `DATUM_*` → drafting crate annotations.

**Success criteria.** Import a multi-body STEP file from McMaster-Carr
(e.g. an SKF bearing or a bracket with mounting hardware) and have the
assembly tree, all trimmed faces, and — for stretch — GD&T render
correctly.

---

### 4. Associative drawings with persistent face/edge IDs

**Labels:** `backlog`, `kernel`, `drafting`

**Background.** `crates/vcad-kernel-drafting/` renders dimensions on 2D
projections, but they aren't bound to topology — any parameter change
re-evaluates the feature DAG and drops every dimension. This is the
hardest problem in drafting and nobody in FOSS has solved it. Doing so
is a landmark.

**Approach.**
1. **Persistent topology IDs.** Add stable UUIDs on faces/edges/vertices
   through the feature-DAG evaluator (changes to `vcad-ir` + kernel
   eval loop). Each feature op propagates IDs deterministically based
   on parent topology + feature inputs.
2. **Dimensions reference IDs, not coordinates.** Dimension schema in
   drafting crate stores `(face_id, edge_id)` pairs instead of raw points.
3. **Remap pass.** After regeneration, walk dimensions and re-bind via
   topology-match (handles split/merge by name mangling); flag orphaned
   dims rather than silently drop them.

**Scope v1.** Linear / radial / angular dimensions survive a parameter
change on the parent feature. Orphan detection UI in the app.

**Reference.** SolidWorks "intelligent" dimensions, Onshape's ID scheme.

---

### 5. BRep-aware diff & merge — "git for CAD"

**Labels:** `backlog`, `collaboration`, `ai-native`

**Background.** `vcad-crdt` merges the feature DAG at parameter
granularity (LWW HLCs) — it has no geometric awareness. Two branches
that are parametrically different but geometrically equal look like
conflicts; concurrent feature additions that geometrically overlap
aren't detected as conflicts. Uniquely aligned with vcad's AI-native
identity, and already ~70% built.

**Deliverables.**
- New crate `crates/vcad-kernel-diff/`:
  - `diff_brep(a: &Solid, b: &Solid) -> BRepDiff` — added / removed /
    modified faces with volume + surface-area deltas.
  - Topology-aware matching (persistent IDs from #4, or Hausdorff
    fallback if IDs unavailable).
- App: side-by-side compare view with coloured overlay
  (red=removed, green=added, yellow=changed).
- Use diff as merge-conflict detector in `vcad-crdt`.
- Hook into Supabase `document_versions` — "Compare with v12" button.

**Use cases.** PR reviews on `.vcad` files, Supabase version-history
visual compare, AI edit validation.

**Reference.** NX "compare bodies", Onshape "compare workspaces".

---

### 6. Sheet metal workspace

**Labels:** `backlog`, `kernel`, `app`

**Background.** Entirely absent in both kernel and app. High-volume
mass-market use case (enclosures, brackets, chassis) that differentiates
vcad from most FOSS CAD systems.

**Deliverables.**
- Kernel: new `crates/vcad-kernel-sheetmetal/` with:
  - Flange (edge → bent flange with radius, angle, length).
  - Bend / Unbend (split at bend, unfold / refold).
  - Flat pattern (part → flat DXF with bend lines + K-factor annotations).
  - K-factor table (material library, configurable).
  - Corner reliefs (tear, obround, rectangular).
- App: new workspace mode in `packages/app/src/components/sheetmetal/`
  with flange tool, bend tool, unfold toggle, flat-pattern export button.
- MCP tools: `create_flange`, `unfold_part`, `export_flat_pattern_dxf`.
- Tests: standard enclosure, L-bracket, box-with-lid round-trips.

**Success criteria.** Draw a flat plate → add 3 flanges → unfold →
export DXF → re-fold gives identical geometry.

---

## IDEAS (future discussion — not queued)

### Direct / synchronous modeling
Face-level push/pull on imported dumb bodies with intent inference
(planar, cylindrical, pattern-aware). Unblocks editing of STEP imports
without a feature tree. High effort; high payoff. Blocked on #2.

### Multi-face corner fillets + variable radius
`vcad-kernel-fillet` handles edge blends (plane-plane, plane-cyl,
cyl-cyl, general curved via NURBS rolling ball). Missing: 3+ face
non-tangent corners, per-edge variable radius profiles, face blends.

### Sub-D / T-Splines modeling
Subdivision-surface modeling integrated with BRep (Fusion's "Form"
workspace). No subdivision code exists. Legendary-tier differentiator
but very large effort.

### Mesh-to-BRep reverse engineering
Scan-to-CAD: fit parametric surfaces to mesh input. No code exists.
Legendary-tier; requires RANSAC surface fitting + region growing +
topology reconstruction.

### Weldments
Structural members along sketches, corner treatments, gussets, trim /
extend, weld beads, cut lists. Missing entirely.

### CAM 5-axis toolpaths + additional post-processors
`vcad-kernel-cam` has 2.5D (Face, Pocket2D, Contour2D, Roughing3D) with
GRBL / LinuxCNC posts. Missing: tool-axis orientation, simultaneous
5-axis finishing, Haas / Fanuc / Heidenhain posts.

### Mold design workspace
Parting-surface extraction, core/cavity split, draft analysis, ejector
pins, gate/runner layout. Missing entirely.

### GD&T / PMI in 3D viewport
`vcad-kernel-drafting` has `FeatureControlFrame`, `DatumFeatureSymbol`,
`ToleranceMode` — but only 2D. Needed: attach FCFs to faces in 3D,
semantic tolerance query API, STEP PMI round-trip (see #3).

### FEA (static / modal / thermal)
No finite-element code. Could be in-kernel (ambitious) or wrapper around
Calculix / MFEM (pragmatic). Mesh generation from BRep is the hard part.

### Topology optimization → editable BRep
Mesh→BRep (see above) is the blocker. Until that exists, topology-opt
outputs are only useful for STL export.

### Tolerance stack-up (Monte Carlo)
No tolerance propagation. Underserved in FOSS. Low-effort win once
dimensions are associative (#4) and GD&T is queryable.

### Mechanism sim with friction / backlash / compliance
`vcad-kernel-physics` uses Rapier3D with PD-motor joint control only.
Missing: contact friction tuning, joint backlash, compliance springs,
soft bodies.

### Parametric configurations / design tables
IR already supports feature parameters. Needed: named configurations,
suppressed-feature state per config, Excel-like variant table UI.

### Scripting API surface
Loon (`cad-lib/src/lib.loon`) is compile-time; MCP is agent-driven.
Missing: end-user JavaScript or Python plugin API with event hooks.

### VR/AR review (WebXR)
No WebXR hooks. Low-effort flashy demo; low daily-driver value.

### G2/G3 surface continuity, zebra analysis, curvature combs
NURBS evaluator exists (`vcad-kernel-nurbs`, 1.3K LOC); no continuity
solver or visualisation hooks. Required for class-A surfacing.

### Large-assembly perf — instancing for 10K+ parts
LOD generation (`packages/engine/src/gpu.ts`) + mesh cache exist; no
verified scaling test for thousands of instances.

### Multi-part ray tracing + material editor
`RayTracedViewport` currently renders only the first part with a solid.
Extend to multi-part + material library.

### Cross-document undo/redo
Sketch-local and document-level undo works; cross-document / branching
history does not.

### STEP PMI pass-through
Folded into #3 as stretch; breaking out if #3 ships without it.

### Semantic BRep search via MCP
`inspect_cad` returns volume/area/mass; no feature-type queries.
Folded into #2 — if #2 ships the `find_features` MCP tool, this is
covered.

### AI feature recognition UX polish
Once #2 lands, surface results in the app: clickable feature chips,
"find similar", bulk re-selection. Pure UX work.

### Real-time collab OT conflict UI
Participant cursors and CRDT merge exist; no explicit conflict-resolution
UI when two users edit the same parameter simultaneously.

---

## Notes

- Backlog items are ordered by dependency / leverage: #1 unblocks
  trustworthy #2 / #3 output; #2 is the widest single unlock; #4 is a
  prerequisite for #5 being truly robust.
- "Ideas" items marked "missing entirely" would each require a new
  crate on the scale of existing kernel crates (1–5K LOC).
- All item IDs reference paths in the kernel + app audits run on
  branch `claude/advanced-cad-use-cases-cdzwd`.
