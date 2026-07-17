# Structural FEA — milestone ladder (M0–M2)

`crates/vcad-kernel-fea` answers the everyday question — **will this bracket
break?** — on the part's real BRep geometry: a linear-elastic solve returning
max von Mises stress, max displacement, and (given a yield strength) a safety
factor, with a fail-closed mesh-convergence gate in front of every claim.

The design follows the house solver pattern (`vcad-kernel-thermal`,
`vcad-kernel-topopt`): a small self-contained crate, closed-form validation,
fail-closed verdicts, a `vcad.<domain>-claims/1` family riding
`vcad-receipt`'s open domain vocabulary, and an MCP tool.

## Relationship to `vcad-kernel-topopt::analyze`

topopt's `analyze_mesh` is the fast trilinear-hex steering loop (the
`predict_physics` tool). This crate is the *audited* path: a conforming
tetrahedral discretization of the tessellated part, an explicit refinement
study, and a verdict that refuses to claim anything a second mesh level does
not corroborate. Same physics, different contract — steer with
`predict_physics`, certify-provisionally with `analyze_structure`.

## M0 — tet mesh + linear elastic solve + closed-form validation ✅

- **Mesh** (`mesh::tet_fill`): the part's tessellated boundary
  (`vcad-kernel-tessellate::TriangleMesh`) is filled with a uniform lattice
  of linear tetrahedra — interior cells found by per-column ray parity
  against the surface (the topopt voxelizer's machinery), each interior cell
  split into six tets with the Kuhn decomposition (face-conforming across
  cells, watertight interior verified by a face-count test). The boundary is
  a staircase at the lattice pitch; that is a *stated approximation* priced
  by the M1 gate, not hidden. A Delaunay-based boundary-conforming mesh is
  the M3+ upgrade.
- **Solve** (`solve::solve_static`): constant-strain-tet linear elasticity,
  matrix-free (per-element shape gradients, ~100 B/tet), Jacobi-PCG at unit
  Young's modulus with rescale-by-1/E (stress is E-independent for
  force-driven loads). Loads and supports select nodes with axis-aligned box
  regions — **fail-closed: a region selecting no node is an error**, never a
  silent no-op. Stopping criterion is relative to the load-vector norm
  (scale-invariant).
- **Validation** (in-crate tests):
  - Axial bar `δ = FL/(EA)`: compliance within **3%** (constant-strain tets
    are near-exact for uniform stress; the max-displacement magnitude reads
    a few % above the axial value because it carries the Poisson lateral
    motion of the tip corners — the test says so).
  - Cantilever 80×10×10 mm aluminum, 100 N tip load, vs Timoshenko
    (bending + shear, δ ≈ 0.301 mm): three-level refinement study converges
    **monotonically from below** (linear tets are too stiff in bending) to
    within ~12% at 96 cells along the length; root-stress location and
    tip-deflection location checked against beam theory.
  - E-scaling identity: displacement ∝ 1/E exactly, stress E-invariant.
  - Mesh conformity: exact volume recovery on lattice-aligned boxes, every
    interior face shared by exactly two tets, all tets positively oriented.

## M1 — convergence gating + safety factor ✅

`convergence::analyze_converged` solves the same case at ≥ 2 lattice
refinements (each 2× the previous, capped at 256 cells along the longest
axis) and reports the inter-level relative change of each QoI as its
discretization-error estimate:

- **Gate**: max displacement change ≤ `displacement_tol` (default 5%) AND
  max von Mises change ≤ `stress_tol` (default 15%; pointwise stress
  converges slower, and at a genuinely singular re-entrant corner it
  *diverges* — the Unverifiable reason says "fillet it", which is design
  feedback, not solver failure).
- **Verdict**: `Converged` or `Unverifiable { reasons }` — fail-closed. An
  unconverged study produces **no** safety factor and **no** claims.
- **Safety factor**: `yield_strength / max_von_mises` from the finest level,
  only on a converged study, with the smearing caveat attached (the peak is
  a lower bound, so the factor is optimistic near sharp corners).

## M2 — claims + MCP wiring ✅

- **`vcad.fea-claims/1`** (`receipt` module): `max_von_mises_mpa`,
  `max_displacement_mm`, `safety_factor`, plus the two
  `discretization_error_*_rel` conscience claims. Full solver provenance
  (per-level grid/tets/nodes/PCG stats, material constants, the entire
  load/support set). Every note states the missing physics: linear
  elasticity only, no plasticity/buckling/contact/dynamics, staircase
  boundary, smeared concentrations.
- **Unified receipt**: `design_claims` maps the set onto `vcad.receipt/1`
  with `basis: Predicted` — a receipt built from them rolls up
  **Provisional, never Pass** (the thermal/particle contract). An
  Unverifiable study instead emits a single `structure.convergence` claim
  with verdict `unverifiable` and the reasons, so a receipt including the
  analysis can never quietly pass.
- **MCP**: `analyze_structure` (packages/mcp `tools/structure.ts`, wired
  like `solve_thermal`): takes `document_id` + `part` (the part's evaluated
  mesh via `resolvePartMesh`), loads/supports as world-frame box regions,
  material constants, resolution/levels/tolerances. WASM binding
  `feaAnalyzeMesh` in `vcad-kernel-wasm`; engine wrapper
  `Engine.feaAnalyzeMesh`. Finest-level resolution capped at 160 for the
  MCP tier.

## M3+ — out of scope for this pass

Deliberately not started, in rough order of value:

- **Adjoint gradients** (`d(max stress)/d(parameter)` via the discrete
  adjoint; the max-QoI needs the thermal crate's smooth-max treatment) and
  seam registration with `vcad-kernel-diff`.
- **Boundary-conforming Delaunay tet mesh** (snap lattice boundary nodes to
  the surface / true CDT) — removes the staircase, sharpens stress at
  surfaces, and makes face-selection exact instead of box-region-based.
- **Face-id load/support selection** — bind loads to BRep face ids from the
  document instead of world boxes, surviving parameter edits.
- Richardson extrapolation of QoIs (report the h→0 estimate, not just the
  finest level), multi-material assemblies, gravity/body loads,
  modal analysis, Euler buckling estimate.
