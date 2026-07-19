# Unified Analyze Mode (#592)

One shell for all solver domains in the web app, receipts mandatory. V1 ships
two study types — **structural FEA** (`vcad-kernel-fea`) and **tolerance
stackup** (`vcad-kernel-tolerance`) — behind a shared, extensible "study"
abstraction so later domains (thermal, EM, acoustics, clearance) land as new
study types, not new panels.

## Principles

- **Fail-closed receipts.** No result is rendered without its claim status.
  Every solver already emits `ReceiptClaim[]` (`vcad.fea-claims/1`,
  `vcad.tolerance-claims/1`) with `basis: "predicted"`; the UI badge is
  therefore **Provisional** at best until a real measurement closes the loop.
  A study whose spec no longer matches the geometry shows **Stale**; a failed
  requirement shows **Violated**; a re-run that reproduces the stored result
  within tolerance shows **Holds** (same semantics and epsilon discipline as
  `check_clearance` / `verify_receipt` in `packages/mcp/src/tools/clearance.ts`).
- **Viewport stays live.** Solves run in a dedicated web worker with its own
  kernel-WASM instance (modeled on `packages/engine/src/eval-worker.ts`).
- **Studies are document data.** They persist on the `.vcad` document and
  re-verify when geometry changes.

## IR (persistence)

New types in `crates/vcad-ir` (ts-rs exported, mirrored into
`packages/ir/src/generated.ts` via `npm run ir:gen`):

```
AnalysisStudy { id, name, study: AnalysisStudyKind, baseline?: AnalysisBaseline }
AnalysisStudyKind =
  | Structural { resolution, youngs_modulus_mpa, poisson, yield_strength_mpa?,
                 loads: [StudyLoad], supports: [StudySupport] }
  | Tolerance  { contributors: [StudyContributor], requirement: StudyRequirement }
StudyLoad      { region: StudyRegion, force: [f64;3] }
StudySupport   { region: StudyRegion, fix: [bool;3] }
StudyRegion    { min: [f64;3], max: [f64;3] }   // world-frame AABB (kernel contract)
StudyContributor { name, coeff, nominal, tol_minus, tol_plus, dist }
StudyRequirement { name, lower_mm?, upper_mm? }
AnalysisBaseline { recorded_at_iso, quantities: [ {id, value, unit} ] }
```

`Document.analysis_studies: Vec<AnalysisStudy>` follows the
`clearance_specs` pattern (`#[serde(default, skip_serializing_if = …)]`).

Regions are world-frame AABBs because that is the kernel's fail-closed node
selection contract (`FeaSpec::RegionBox`) and stable topological face names
are not yet plumbed to the TS side (#573 follow-up). Face picks in the UI are
converted to the picked face's AABB (inflated by a small epsilon along its
normal). When a region selects no node the solver errors — surfaced as
**Stale** ("re-pick faces"), never silently dropped.

### CRDT survival

The web app document is CRDT-canonical (`VcadFileCrdt` v0.4). Unlike
`clearance_specs` (server-JSON only today), studies get a CRDT seat:
a singleton `analysis-studies` feature (JSON-blob param, exactly the
`molecule` / `scene-settings` pattern) —

- `crates/vcad-app/src/feature.rs`: `FeatureInput::AnalysisStudies { studies: Option<String> }`
- `crates/vcad-app/src/materializer.rs`: `materialize_analysis_studies` →
  `doc.analysis_studies`
- TS: `getOrCreateAnalysisFeature` + `set_param(fid, "studies", crdtStr(json))`
  in the analyze store, mirroring `setCrdtSchematic`.

## Kernel / WASM

`feaAnalyzeMesh` today returns scalar summaries only — no field to color the
mesh with. Changes:

1. `vcad-kernel-fea::solve`: new `solve_static_full` returning
   `FullSolution { summary: Solution, node_displacement_mm: Vec<f64>,
   node_von_mises_mpa: Vec<f64> }` (per-node von Mises = volume-weighted
   average of incident element stresses). `solve_static` stays as a thin
   wrapper.
2. `vcad-kernel-fea::convergence`: `analyze_converged_fields` also returns the
   finest level's `NodeFields { nodes, displacement_mm, von_mises_mpa, h_mm }`.
3. `vcad-kernel-wasm::feaAnalyzeMesh`: options gain `fields: bool` (default
   false — MCP path unchanged). When set, nodal fields are sampled onto the
   *input surface vertices* by nearest-lattice-node lookup (uniform spatial
   hash at pitch `h`), returned as `vertex_displacement_mm: Float64Array` and
   `vertex_von_mises_mpa: Float64Array` aligned with `positions`.

`toleranceAnalyze` needs no kernel change (no field; results card only).

## Worker protocol (`packages/engine/src/analyze-worker.ts`)

Modeled on `eval-worker.ts`; owns its own kernel-WASM instance.

```
→ { type: "init" }                                  ← { type: "ready" }
→ { type: "fea", id, specJson, optionsJson, positions, indices }  // transferables
                                                    ← { type: "result", id, analysis }  // incl. vertex fields
→ { type: "tolerance", id, specJson, paramsJson, optionsJson }
                                                    ← { type: "result", id, analysis }
any error                                           ← { type: "error", id, message }
```

Client wrapper `packages/engine/src/analyze.ts` exposes
`runStructuralStudy(spec, mesh)` / `runToleranceStudy(spec, params)` as
promises, one in-flight solve per study (superseded requests cancelled by id).

## App

- **Store** `packages/app/src/stores/analyze-store.ts` (zustand):
  `active` (drives an `Analyze` entry in `useAppMode`), `studies` (mirror of
  `document.analysis_studies`), `draft` (setup flow state: study type, picked
  regions, magnitudes), per-study `runs: { status: idle|running|done|error,
  result, claims, claimStatus }`, `fieldOverlay: { studyId, field:
  "displacement"|"vonMises" } | null`. Mutations write through the document
  store to the CRDT feature.
- **Setup flow**: pick study type → for structural, click faces in the
  viewport (existing raycast face info from `SceneMesh` / selection store;
  each pick becomes a `StudyRegion`) → set magnitudes in a properties-style
  panel (reuses `InlineProperties` input patterns).
- **Results**: a results card per study — QoIs, convergence verdict, and the
  mandatory **claim status ribbon**. Field overlay writes a colormap
  (viridis-like ramp over the selected field) into `mesh.colors`
  (`TriangleMesh.colors` → `SceneMesh` already renders per-vertex colors).
- **Claim status** (fail-closed):
  - solver returned `Unverifiable` → **Unverifiable**, no QoIs colored, reasons shown;
  - claims present, `basis: predicted` → **Provisional**, with the
    "what closes it" line from the claim's oracle/description (e.g. "measure
    tip displacement under the rated load");
  - stored baseline reproduced within tolerance on re-run → **Holds**;
  - geometry changed since last run (document revision bump, or a region now
    selects nothing) → **Stale** until re-run;
  - requirement violated (safety factor < 1, tolerance requirement failed) →
    **Violated**.
- **Wiring**: `AnalyzePanel` mounted from `App.tsx`; `Analyze` added to
  `useAppMode` below Physics priority; toolbar toggle.

## Extensibility

A study type contributes: an `AnalysisStudyKind` variant (IR), a worker
message, a setup form, a results card body, and a claim mapper. The shell
(store, worker plumbing, claim ribbon, persistence) is shared. Thermal is the
natural next tenant (`thermalSolve` already exists in WASM).

## Non-goals (v1)

- Measured-basis closure (uploading a real measurement to flip Provisional →
  Pass) — MCP `record_measurement` remains the path.
- Stable topological face references in studies (#573 plumbing to TS).
- Per-element result probing / section cuts.
