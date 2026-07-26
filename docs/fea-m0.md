# Structural FEA — milestone ladder (M0–M3)

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

## M3 — thin walls: measure, diagnose, and answer in closed form ✅

Found in the field (2026-07-26, sizing a robot chassis): the lattice route is
unusable on thin-walled parts, which is most of a sheet-metal or tube-frame
machine. The arithmetic is not a corner case, it is the domain:

```
160 cells over a 312 mm member  ->  1.95 mm/cell  ->  ~1 cell through a 2 mm wall
6 cells through 2 mm            ->  0.33 mm/cell  ->  ~950 cells, past every cap
```

A staircase at one cell per wall is not a coarse approximation of a thin wall;
it is a different part. Raising `resolution` is not the lever — the mesher's own
hard ceiling of 256 is still ~4× short — so this milestone does two things
instead.

**Measure, then diagnose** (`mesh::wall_thickness`, `mesh::diagnose_thin_wall`).
Axis-aligned rays on a 32×32 grid per axis collect the solid spans through the
part; the 5th percentile of that distribution is the working estimate of the
thinnest load-bearing section (exact for plate and prismatic-tube geometry,
which is the reported domain; the percentile rather than the minimum keeps
grazing rays at curved surfaces from dominating). `diagnose_thin_wall` turns
that into the cell arithmetic — pitch, cells through the section, the
resolution ~6 cells would need, whether that is under the caller's cap — and
below 4 cells emits `blocking_advice`, which:

- forces the convergence verdict to `Unverifiable` **even when the QoIs agree
  between levels** (a study that never resolved the wall can agree with itself
  and still be wrong), and
- replaces the old bare `NoInteriorCells` ("raise the resolution" — advice that
  cannot work here) via `ConvergenceError::ThinWalled`, which carries the
  diagnosis.

Between 4 and 6 cells the study runs and the gate judges it, with an `advisory`
recorded in the claim provenance. Converged studies now state
cells-through-section in their provenance regardless — the arithmetic is on the
record either way. Cost is a small fraction of one solve.

**Answer it in closed form** (`section`). For a *prismatic* member the closed
form is not a consolation prize, it is the better answer:

- `Profile`: `rect`, `rect_tube`, `round`, `round_tube`, `i_beam` (outside
  dimensions, uniform wall). Properties: `A`, `I_y`, `I_z`, section moduli,
  Saint-Venant `J`, torsional modulus `T/τ_max`, transverse-shear factor,
  Timoshenko κ.
- Torsion by provenance, never by table lookup: **exact** for round and round
  tube (`J = 2I`, any wall); the **convergent Saint-Venant Fourier series** for
  solid rectangles (reproduces the classical `J = 0.1406 s⁴`, `τ = T/(0.208 s³)`
  square-bar constants, and Roark's 2:1 values to 0.5%); **Bredt closed
  thin-wall** for rectangular tube (`J = 4A_m²t/s`, `τ = T/(2A_m t)`); the
  thin-strip Saint-Venant sum for the open I-section, which states that it
  ignores warping entirely.
- `check_beam`: bending moment and stress by end condition (six cantilever /
  simple / fixed-fixed cases, point and distributed), axial stress, torsional
  and transverse shear superposed conservatively, von Mises, deflection with
  the Timoshenko shear term, twist, torsional stiffness, Euler buckling under
  compression (`K` from the end condition), safety factor.
- **Fail-closed applicability**, same contract as the lattice gate:
  `L/depth < 5`, Bredt on a wall thicker than 0.2 of the section, deflection
  past `L/10`, torque on an open section, or an Euler margin below 1 → verdict
  `Unverifiable`, no QoI claimed, and each reason names the route forward
  (usually back to `analyze_structure`, since a stubby part is exactly what the
  lattice *can* resolve). Non-blocking `cautions` (short-but-workable
  slenderness, `d/t > 50`, buckling margin under 2) ride the provenance.
- Claims land on the same `vcad.fea-claims/1` schema under receipt ids
  `structure.beam.*` with oracle `vcad-kernel-fea/section` and
  `basis: predicted` — exact arithmetic on an idealized member is still not a
  load test, so receipts roll up **Provisional**.
- Validation: the same 80×10×10 aluminum cantilever the lattice is validated
  against (0.301 mm Timoshenko tip deflection, 48 MPa root stress, within 2%
  and 1%), the classical torsion constants above, hand-computed Bredt values
  for the 40×40×2 chassis tube, and a slender strap that yields comfortably
  while Euler says it folds (verdict `Unverifiable`, not "safe").

**MCP**: `beam_check` (`tools/structure.ts`), WASM binding `feaCheckBeam`,
engine wrapper `Engine.feaCheckBeam`. It takes **no document** — geometry by
description, so it works before the part exists.

What this does *not* cover: a non-prismatic thin-walled part (a bent sheet
bracket with cutouts) still has no audited answer. Shell and beam *elements*
remain the real fix, and are the top of the M4+ list below.

## M4+ — out of scope for this pass

Deliberately not started, in rough order of value:

- **Shell and beam elements** — the physically right discretization for plate
  and thin-walled geometry, sidestepping the through-thickness cell problem for
  *any* shape rather than only prismatic ones. Biggest job, biggest payoff: it
  covers the whole sheet-metal-robot domain that M3's closed forms only reach
  when the member happens to be prismatic.
- **Prismatic-section detection from a solid** — verify a constant cross-section
  by sampling stations along the longest axis, then derive `A`/`I` by
  integrating the rasterized section, so `beam_check` can be handed a part
  instead of a profile. `J` for an arbitrary section needs a warping (Poisson)
  solve, so torsion would stay profile-driven until that lands.
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
  modal analysis. (Euler buckling landed in M3 for prismatic members.)
