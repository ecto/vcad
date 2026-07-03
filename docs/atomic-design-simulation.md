# Atomic & molecular design and simulation — vision + roadmap

The plan for making vcad a legendary place to design, simulate, and render
matter at atomic resolution — not a bolt-on molecule viewer, but a domain that
exploits the three things only vcad has in one repo: **one parametric document**,
**one differentiable math stack (`tang`)**, and **one photoreal renderer**, all
driven by **one AI-native interface (MCP)**.

## The bet

Every existing tool is *forward*: specify a structure, get a property (VMD +
LAMMPS, PyMOL + GROMACS, Materials Studio). vcad is the only stack positioned to
run it *backward* — **inverse design of matter, end-to-end differentiable**.
You state the property you want (band gap, binding energy, stiffness, a catalytic
site geometry), and gradients flow from that target, through the simulation, back
to atomic coordinates *and the parametric design variables that generated them*.
An AI agent closes the loop over MCP. Everything below serves that bet.

Table stakes (a viewer + a classical MD engine) already exist elsewhere and are
better than anything we'd ship in v1. The legendary parts are the ones nobody
else can build because nobody else has the CAD kernel, the differentiable stack,
and the renderer in the same process.

## What only vcad can do

1. **Multiscale in one document.** A `Document` already carries orthogonal
   domains as optional fields (assembly, ECAD). Atoms become another. Design a
   macroscopic part (BRep), an assembly, *and* an atomic region in the same DAG,
   same agent, same viewport — continuous from millimeter to Ångström.
2. **Differentiable across the scale boundary.** `tang-ad` is reverse-mode
   autodiff; `vcad-kernel-diff` already propagates `dx/dθ` through a frozen
   tessellation with `tang-optim`. Reuse that harness so gradients flow from an
   atomic-scale objective into the *CAD parameters* (lattice constant, pore
   radius) that generated the atoms.
3. **Near-DFT fidelity without curating force fields.** ML interatomic
   potentials (MACE-MP, Orb, NequIP/Allegro) give near-DFT accuracy at MD speed
   with no hand-tuned parameters. `tang` already ships `tang-onnx` /
   `tang-infer` / `tang-train` — the inference substrate is in-house. This
   dissolves the classical-force-field parameter-curation problem.
4. **Rendering that looks designed.** The ray tracer already does Cook-Torrance
   PBR, soft shadows, one-bounce GI, SSAO, IBL, and Fusion-style edges. Point it
   at impostor spheres, solvent surfaces, and density isosurfaces and molecules
   come out publication-grade by default — straight out of MCP.
5. **Reproducibility as a feature.** The parametric DAG means every structure is
   reconstructable from its construction, and `build_receipt` / `verify_receipt`
   already exist. Every simulation becomes cryptographically reproducible and
   citable.

## Architecture

```
                          Document  (crates/vcad-ir/src/lib.rs:1604)
                                │  + molecule: Option<MoleculeSystem>   (new, optional)
                                ▼
     ┌──────────────────────────────────────────────────────────────────┐
     │                     vcad-kernel-atoms   (new crate)                │
     │   MdWorld::from_document(doc)  — reads `molecule` directly,        │
     │   no CSG evaluation (mirrors vcad-kernel-physics/src/world.rs)     │
     │                                                                     │
     │   integrator (velocity-Verlet, thermostats, PBC, neighbor list)    │
     │        force()  ┌──────────────┬──────────────────────────┐        │
     │                 │ classical FF │ ML potential (tang-infer) │        │
     │                 │  (tang-ad)   │  MACE/Orb via tang-onnx   │        │
     │                 └──────────────┴──────────────────────────┘        │
     │   minimize()  → tang-optim (L-BFGS / LM)                           │
     │   FD oracle   → vcad-kernel-diff `fd` pattern (forces == −∇E)      │
     └──────────────────────────────────────────────────────────────────┘
              │ WASM (#[wasm_bindgen] MdSim, feature-gated)                │ native
              ▼                                                            ▼
   packages/engine/src/atoms.ts            headless-wgpu render harness (new)
   (mirrors physics.ts)                     drives raytrace Track B → PNG
              │                                                            │
              ▼                                                            ▼
   packages/mcp  tools/atoms.ts        ◀─────────  render_molecule (MCP)
   create_molecule / load_structure / minimize_energy / md_run /
   md_observe / inspect_molecule / design_material (inverse loop)
              │
              ▼
   packages/app  ViewportContent.tsx
   Track A: <AtomInstances> InstancedMesh + impostor ShaderMaterial
   Track B: RayTracer.uploadAtoms(...)  (analytic spheres, own BVH)
```

## Architectural decisions

1. **Atoms are not BRep.** Never model an atom as an IR `Sphere` node + pattern.
   That routes each atom through boolean merges, hits the ray tracer's
   `MAX_SURFACES=1024 / MAX_FACES=4096` caps
   (`crates/vcad-kernel-raytrace/src/gpu/buffers.rs:13`), and is rejected by the
   `render_view` complexity guard (`packages/mcp/src/tools/render.ts:73`). Use a
   lightweight structure-of-arrays store consumed directly by the sim and render
   tracks.
2. **Own unit convention: Ångström.** The geometry/physics stack is millimeters
   with mm→m converters everywhere. The molecular domain declares Å and keeps
   its boundary explicit — do not reuse the CAD converters.
3. **The IR field is the extensibility surface.** `Document.molecule:
   Option<MoleculeSystem>` with `#[serde(skip_serializing_if)]` + `#[serde(default)]`,
   exactly like `instances` / `schematic`. Old docs deserialize unchanged; no
   version bump. Enum variants derive `ToolSchema` so species/bond types
   auto-surface to MCP.
4. **`vcad-kernel-physics` is the crate template, `tang` is the substrate.**
   Mirror its shape (`lib.rs` re-exports, `world.rs`-analog reading the IR field,
   `gym.rs`-style env). Do **not** depend on phyz — atoms are point masses with
   pairwise/bonded potentials; `tang` + `tang-ad` + `tang-optim` suffice.
5. **MLIP fidelity over hand-curated force fields.** Classical FF ships first for
   speed and gradient-testing; the credible-science tier is a pretrained
   foundation potential run through `tang-infer`.
6. **QM stays external.** DFT / ab-initio (PySCF, Psi4, xtb, ORCA) are
   process/service adapters, not reimplementations — they break the in-process
   WASM model and hang off the `packages/mcp/src` remote/http transport.
7. **Two render tracks, matching the existing split.** Track A (three.js
   `InstancedMesh` + impostors) for interactive; Track B (raytraced analytic
   spheres) for quality. MCP PNG needs a *new* headless path — none exists today.

## Milestones

Each milestone lands a demoable payoff and carries its own acceptance gate, in
the spirit of the `differentiable-seam` series (the test *is* the gate).

### M0 — The `molecule` domain + structure I/O

**Goal:** a real structure loads, round-trips, and measures.

- `crates/vcad-ir`: add `MoleculeSystem` (species table: element, mass, charge,
  vdW/covalent radius, CPK color; SoA atom store `positions: Vec<[f64;3]>`,
  `species_idx: Vec<u32>`; bonds as index pairs + order; optional periodic cell)
  and `Document.molecule: Option<MoleculeSystem>`. Regenerate TS via
  `npm run ir:gen`.
- `crates/vcad-kernel-atoms` (new): `io` module — `.xyz`, PDB, CIF importers
  (parallel to `vcad-kernel-urdf`, `vcad-kernel-physics/src/stl.rs`); bond
  perception by covalent-radius overlap.
- MCP: `load_structure`, `inspect_molecule` (atom/species counts, formula,
  bounding cell, radius of gyration).

**Gate:** load a PDB and a CIF, serialize the `Document`, re-load, assert atom
positions and bonds are bit-identical; `inspect_molecule` reports correct formula
and Rg against a known reference.

### M1 — Track A interactive viewport

**Goal:** orbit a real structure at 60 fps.

- `packages/app`: `<AtomInstances>` sibling to `SceneMesh` in
  `ViewportContent.tsx`, gated by a `useUiStore` flag (mirror the ray-traced
  overlay toggle). One icosphere `InstancedMesh` (`setMatrixAt`/`setColorAt`,
  copying `PcbViaMesh.tsx`); bonds as a second instanced cylinder mesh; CPK
  coloring; representation toggle (ball-and-stick / space-filling / wireframe).
- Stretch: **impostor spheres** — a `ShaderMaterial` in `src/shaders/` that
  ray-traces a sphere with correct depth over instanced quads, for 10⁷-atom
  scenes.

**Gate:** render a ≥10⁵-atom crystal supercell interactively above 30 fps;
representation + coloring switch live.

### M2 — Classical MD & minimization (`vcad-kernel-atoms` core)

**Goal:** correct dynamics you can trust, validated against −∇E.

- Integrator: velocity-Verlet, Berendsen + Nosé-Hoover thermostats, periodic
  boundaries, cell/Verlet neighbor list.
- Classical potential: Lennard-Jones + Coulomb + harmonic bond/angle/dihedral,
  forces via `tang-ad` (`force = −∇E`).
- `minimize()` → delegate to `tang-optim` (L-BFGS/LM); harness modeled on
  `vcad-kernel-diff/src/optimize.rs`.
- **FD oracle from day one:** reuse the `vcad-kernel-diff` `fd` central-difference
  pattern to assert analytic forces match numerical −∇E within tolerance.
- WASM: feature-gated `#[wasm_bindgen] MdSim` (copy the `#[cfg(feature="physics")]`
  + stub pattern, `crates/vcad-kernel-wasm/src/lib.rs:3281`).
- `packages/engine/src/atoms.ts` (mirror `physics.ts`, same
  `serde_wasm_bindgen` → `mapToObject` handling).
- MCP gym verbs: `create_molecule`, `minimize_energy`, `md_run` / `md_step` /
  `md_observe` (4-touchpoint pattern in `server.ts`).

**Gate:** LJ argon reproduces the known melting-transition RDF; energy is
conserved in NVE over 10⁴ steps within tolerance; FD oracle passes for every
potential term.

### M3 — ML interatomic potential force engine

**Goal:** near-DFT fidelity, no parameter curation.

- Pull a pretrained universal potential (MACE-MP / Orb; Hugging Face) and run it
  as the `force()` callback through `tang-infer` / `tang-onnx`.
- Potential abstraction: `trait ForceField { fn energy_forces(&self, sys) -> (f64, Vec<[f64;3]>) }`
  with classical and MLIP implementations behind it.
- Batched/GPU inference path where the model supports it.

**Gate:** MLIP energies/forces on a held-out set match reference DFT within the
model's published error; a relaxation converges to a known equilibrium geometry.

### M4 — The differentiable inverse-design loop (crown jewel)

**Goal:** state a property target; a real structure descends into existence.

- With the integrator and MLIP written in `tang`, the trajectory is one
  differentiable graph. Define losses on properties (elastic modulus, bandgap
  proxy, binding energy) and backprop to coordinates via `tang-ad`.
- **Cross-scale coupling:** reuse the `vcad-kernel-diff` seam so gradients flow
  past coordinates into the *parametric DAG* variables that generated the atoms —
  optimize CAD knobs against an atomic objective.
- Guardrails for differentiable MD: checkpointed adjoints (memory), gradient
  stability over long trajectories, `tang-optim` with box bounds.
- MCP: `design_material(target, constraints)` runs the loop.

**Gate:** on a toy objective (e.g. target lattice constant / nearest-neighbor
distance), the loop drives a design parameter to the analytic optimum;
gradients match FD.

#### M4.5 — The homogenize bridge (landed)

The first cross-scale coupling is live, in the FD-oracle regime (the
`tang-ad` swap-in point is marked at every seam):

- `vcad-kernel-atoms::homogenize` — density plus cubic elastic constants
  (C11/C12/C44 from strain-energy second differences, internal FIRE
  relaxation under each strained cell) reduced to isotropic K/G/E/ν by
  Voigt–Reuss–Hill, packaged as a `MaterialCard` (SI density, GPa moduli).
  Shear sweeps are legal because `potential::min_image` now handles
  general (non-orthorhombic) cells by fractional rounding.
- `vcad-kernel-physics::diff::rollout_gradient_via_density` — the density
  channel of the M8 factorization: `dp/dρ` is exact (mass properties are
  linear in density; the COM does not move), so θ reaches a phyz rollout
  through a material model with no CAD rebuild at all.
- MCP: `homogenize_material(molecule, force field) → MaterialCard`.

**Gate (passing):** `crates/vcad-kernel-physics/tests/cross_scale.rs`
computes `dJ/d(lattice constant)` for an argon-FCC-densified flywheel —
Å → kg/m³ → seam mass properties → phyz rollout — and matches both the
brute-force whole-chain central difference and the closed form `3J/a` to
1e-4. The atoms-side gates in `homogenize::tests` include the Cauchy
relation `C12 = C44` for a pair potential at zero pressure.

### M5 — Legendary rendering

**Goal:** publication-grade images out of MCP.

- **Solvent surfaces** (SES/SAS) as *real geometry* via a rolling-ball offset —
  a Minkowski/offset op the BRep kernel understands — ray-traced with the same
  shading as a machined part.
- **Density isosurfaces / molecular orbitals** via marching cubes, borrowing the
  octree-SDF + marching-cubes machinery in `vcad-kernel-stocksim`; volumetric
  field rendering on top.
- **Track B raytraced impostors:** dedicated sphere-instance buffer + its own
  BVH over sphere AABBs in `GpuScene` (reuse `bvh.rs` SAH over centroids),
  traversed alongside the face BVH in `raytrace.wgsl`; `RayTracer::uploadAtoms`
  parallel to `upload_solid` (`crates/vcad-kernel-wasm/src/lib.rs:2799`). Lift
  the `MAX_*` caps.
- **Headless PNG for MCP:** new headless-wgpu render harness (offscreen device +
  readback) exposed as `render_molecule` — the missing piece for world-class
  images server-side. Interim: teach `vcad-render` to rasterize instanced spheres
  for the SVG/PNG path.

**Gate:** `render_molecule` returns a cinematic PBR PNG of a protein with SES +
ball-and-stick, headless, in one MCP call.

### M6 — The autonomous discovery loop + reproducible receipts

**Goal:** an agent runs the whole loop and its results are verifiable.

- Wire the atomic tools so an agent can iterate propose → minimize → simulate →
  observe → render → mutate, steering along M4 gradients rather than random
  search (the `verify_part` self-grading pattern is the template).
- Every simulation emits a `build_receipt`; `verify_receipt` re-runs it for
  cryptographic reproducibility.
- QM adapters (xtb/PySCF) as external services over the MCP remote/http transport
  for spot-checking MLIP results.

**Gate:** an agent, given a property target, autonomously returns a candidate
structure with a receipt that a fresh session re-verifies to the same energy.

## Sequencing & the one bet

Build order is M0 → M6 as written; each is independently shippable.

- **Highest leverage, most achievable:** M3 (MLIP force engine) and M6 (agent
  loop) — integration on substrate we already own; either alone is a "nobody
  else ships this" story.
- **The frontier bet:** M4 (end-to-end differentiable inverse design through the
  DAG). Genuinely research-grade — differentiable MD is finicky (long
  trajectories, gradient stability, adjoint memory) and cross-scale gradient
  coupling is unproven. Highest risk, and the thing that makes this *legendary*
  rather than merely excellent.
- **Makes people believe it:** M5 (rendering) and M6 (receipts) — the demo and
  the paper.

**If we bet on one thing:** land M3 so the physics is credible, then make M4 +
M6 the headline — *describe the material property you want and watch an AI
gradient-descend a real, reproducible structure into existence, rendered
beautifully*. No other tool on earth can currently say that sentence.

## Honest reality check

- **The headless raytrace harness does not exist today.** The ray tracer runs
  only in-browser via WASM; `render_view` uses `vcad-render` (SVG→PNG). World-class
  MCP images (M5) require building that harness or accepting SVG-quality interim.
- **Differentiable MD is the risk.** M4 is where the ambition concentrates and
  where a spike should happen early to de-risk before committing the full loop.
- **QM is not ours to reimplement.** Treat it as orchestration + visualization of
  an external engine, always.
- **Units and the mm/Å boundary** are a persistent footgun; keep the molecular
  domain's conversions isolated and tested.
