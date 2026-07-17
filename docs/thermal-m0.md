# Heat-conduction FEA M0: voxel finite volumes, energy-balanced, with θ figures of merit

`vcad-kernel-thermal` makes vcad answer the question every enclosure, motor
driver, PSU, and PCB design asks — *how hot does it get?* — with a defensible
number instead of a hand rule and a safety factor. The loop is the same one
every domain crate in this repo runs:

> geometry + materials + boundary conditions → temperature field → figure of
> merit (T_max, θ) → a claim on a receipt

The incumbent practice is oversizing plus folklore ("1 W per square inch is
fine"); the incumbent tools are either spreadsheet resistor networks (no
geometry) or full CFD (a different profession). This crate is the M0 of a
conduction solver that lives inside the kernel next to the geometry it
scores, priced for the adjoint from day one — the conduction operator is
self-adjoint, so gradients of T_max will cost one extra linear solve (M2).

## M0 scope (and honesty)

**In scope:** steady-state heat conduction on a uniform voxel grid.

- Geometry painted onto the grid as axis-aligned boxes and cylinders/tubes
  (painter's order, later wins); unpainted voxels are void and excluded from
  the system. The tessellated-part voxelizer is the M3 seam — M0 is
  deliberately self-contained.
- Finite-volume discretization with **harmonic-mean face conductances**:
  the face between voxels P and N conducts G = A / (d/2k_P + d/2k_N) — the
  two half-cells in series. This is the standard interface treatment and it
  is *exact* for layered 1D composites. The arithmetic mean is wrong at
  interfaces: it lets the high-k side short across the face (a copper|air
  face would read ~4 orders of magnitude too conductive). The scheme is
  conservative by construction — heat leaving a voxel through a face enters
  its neighbor exactly.
- Boundary conditions: fixed temperature (Dirichlet, with the half-cell
  resistance to the surface), convection h·(T − T∞) (Robin, film in series
  with the half-cell, so h → ∞ recovers Dirichlet), adiabatic. Applied per
  domain face and to exposed solid↔void surfaces; volumetric
  fixed-temperature reservoirs pin voxels directly.
- Power sources: total watts per region, split over the covered free voxels.
  Negative power allowed (TECs).
- Solver: hand-rolled Jacobi-preconditioned conjugate gradients, matrix-free
  stencil apply. Stopping criterion ‖r‖ ≤ tol·‖b‖ — **relative to the
  right-hand-side norm** (the particle-crate lesson: absolute epsilons read
  small-scale problems as converged at iteration zero).
- Outputs: the temperature field, T_max and its location, per-source
  θ = (T_source,max − T_ref)/P, per-reservoir heat flows, and an **energy
  balance whose residual is reported on every solution** — power in vs
  boundary heat out, closing to solver tolerance or the number says so.
- Fail-closed everywhere: a source covering no conducting voxel, a reservoir
  pinning nothing, a floating region with no path to any temperature
  reference, an unresolvable θ reference, a non-converged CG — all errors,
  never silent defaults.

**Out of scope at M0** (each is a milestone below): transient response and
thermal mass, anisotropic conductivity, radiation, any actual fluid
mechanics — **convection enters only as a supplied film coefficient h, and h
is the biggest uncertainty in every prediction this crate makes** (natural
convection correlations carry ±20–30%; radiation at electronics temperatures
is the same order as natural convection, ~6 W/m²K equivalent at 60 °C over
25 °C ambient). Every claim built on these numbers must carry the h it was
priced at.

**Regime of validity:** conduction-dominated solids with known surface
coefficients — boards, enclosures, heatsink-ish metal, potted assemblies. Do
not read fluid-dynamics conclusions (chimney effects, fan curves, boundary
layers) out of this model; it does not contain them.

## Validation ladder (all in `cargo test -p vcad-kernel-thermal`)

Every rung cites its closed form and states what it proves:

- **Dirichlet slab** → linear profile, exact at voxel centers (~1e-14 °C):
  second differences of a linear field vanish; the half-cell boundary
  conductance closes the end equations exactly.
- **Robin slab** → the series circuit T(x) = q·(1/h + (L−x)/k),
  q = ΔT/(L/k + 1/h), exact at centers for h from 50 to 10⁹ W/m²K — the
  film rides in series with the half-cell and the Dirichlet limit is
  recovered smoothly.
- **Composite two-layer slab** (k = 200 | k = 1) → the series-resistance
  formula q = ΔT/(L₁/k₁ + L₂/k₂), exact at centers (max error 2.8e-14 °C at
  200:1 contrast). This is the harmonic-mean proof: an arithmetic-mean face
  fails this rung by orders of magnitude.
- **Heated slab, adiabatic back, convection front** → T_max − T∞ =
  q·((L − δ/2)/k + 1/h) with the generating-layer correction δ/2 stated
  *exactly* (δ = one voxel): the discrete source voxel balances its total
  power against its face flux, which reproduces the same bookkeeping. As
  δ → 0 this is the textbook q(L/k + 1/h).
- **Cylinder shell** (annulus, both rings pinned) → R = ln(r₂/r₁)/(2πkH)
  and the A + B·ln ρ profile. A voxelized circle is a staircase, so this
  rung is a **convergence statement with quantified error, not an exactness
  statement**: the resistance error must shrink under refinement (it does:
  1 mm → 0.5 mm voxels roughly halves it, landing under 5%), and the
  fine-grid profile must actually be logarithmic (least-squares slope within
  5% of −ΔT/ln(r₂/r₁), RMS fit residual < 1% of the drop). Stair-step
  honesty: the pinned boundaries are only defined to ±half a voxel, and
  absolute resistances on coarse grids inherit that first-order error.
- **3D chip-on-plate energy balance** → power in = boundary heat out to
  well under 0.1% (measured ~1e-11 at default tolerance), maximum principle
  respected (nothing below ambient), hottest voxel inside the source
  footprint, θ between the isothermal-plate floor 1/(hA) and a credible
  spreading penalty.

## Benchmark: hot_chip

`cargo run --release -p vcad-kernel-thermal --example hot_chip`

A 10×10×1 mm silicon-ish die (k = 120) dissipating 2 W, centered on a
100×100×1.6 mm board with copper-plane-equivalent k = 15 (isotropic — see
honesty box in the example), convecting from both large faces at 25 °C
ambient, edges adiabatic. 100×100×13 grid (130k voxels), CG to 1e-8 in ~800
iterations, energy balance ~1e-11:

| h (W/m²K) | T_max (°C) | θ_ja (K/W) |
|---:|---:|---:|
| 5 | 67.45 | 21.23 |
| 10 | 56.48 | 15.74 |
| 15 | 52.29 | 13.65 |
| 20 | 49.87 | 12.44 |
| 30 | 46.92 | 10.96 |

Findings:

1. **The film dominates and the sweep says by how much.** Doubling h from
   5 to 10 buys 11 °C; doubling again buys 6.6 °C — diminishing returns as
   spreading resistance in the board becomes the next bottleneck. This is
   the sensitivity a designer actually needs: *is my problem airflow or
   copper?* Here, below h ≈ 20 it's airflow.
2. **The isothermal-plate floor is 9.26 K/W** (1/(h·2A) at h = 10);
   the solved 15.74 K/W means spreading through the k = 15 board costs
   6.5 K/W on top — the die is 10× smaller than the board, and the board is
   not thick enough to erase that.
3. **These numbers are h-conditional predictions, not measurements.** With
   still-air h ≈ 5–10 and radiation not modeled (~6 W/m²K equivalent — the
   same order!), the honest reading of "T_max at h = 10" is "T_max if the
   combined film+radiation coefficient is 10". The M6 measurement pack binds
   these to thermal-camera and thermocouple data with exactly that caveat.

## Milestone ladder

- **M0 — steady conduction + validation + hot_chip. DONE** (this doc).
- **M1 — transient + anisotropy.** Implicit Euler with per-voxel ρc_p
  (SPD system unchanged — same CG), step response validated against
  lumped-capacitance decay (Bi ≪ 1) and the semi-infinite-solid erfc
  solution; diagonal anisotropic conductivity (per-axis k) for real PCBs
  (in-plane copper ~15–20 vs through-plane ~0.3 W/m·K), harmonic mean per
  axis.
- **M2 — the adjoint.** The conduction operator is symmetric — literally
  self-adjoint — so the gradient of any scalar objective costs **one more
  CG solve with the same operator** (the exact trick the particle crate used
  for its Poisson adjoint). Objective: a smoothed T_max (a hard max is
  non-differentiable; a p-norm with documented p and stated bracketing
  replaces it). Gradients w.r.t. per-region conductivity (harmonic-face
  chain rule), film coefficients, and source powers. FD-validated with the
  frozen-discretization lesson: the grid never re-voxelizes across FD
  probes, and the probes state their h-convergence floor. Geometry
  parameters move the discrete material mask and stay FD until a
  shape-adjoint milestone — say so, don't smooth over it.
- **M3 — the seam.** Serde `ThermalSpec` with named document parameters,
  fail-closed resolution (unbound name = error), `parameter_roles()`
  classifying adjoint vs FD paths; an external voxel-field input so the
  vcad side can voxelize tessellated parts (point-in-solid sampling) and
  feed them in without this crate depending on mesh types; MaterialCard
  hookup documented (`vcad-kernel-atoms::homogenize` conductivity →
  `MaterialRegion`).
- **M4 — receipt claims.** `vcad.thermal-claims/1`: `t_max_c`,
  `theta_ja_c_per_w` per source, `energy_balance_residual`, with full
  provenance (grid, CG tolerance and iterations, BC set, anisotropy state)
  and the missing-physics caveats on every claim note — conduction only, h
  supplied, no radiation. Fail-closed like the particle claims: nothing
  defaulted silently.
- **M5 — benchmark + convergence + paper draft.** A published JEDEC-style
  θ_ja comparison (JESD51 boards are exactly this geometry family) with the
  package-model caveat stated, a grid-convergence table that names its
  floor, `docs/thermal-paper-draft.md`.
- **M6 — measurement pack.** `compare()` binding thermal-camera and
  thermocouple measurements to predicted claims with Holds / Violated /
  Unmeasured verdicts, fail-closed (an unmeasured receipt never passes);
  emissivity (camera reads ε·T⁴-ish, boards are ε ≈ 0.9 but bare copper is
  ε ≈ 0.05 — a shiny plane reads *cold*), thermocouple contact resistance,
  and the h-uncertainty band spelled out.

## Non-goals

This crate does not do CFD and will not pretend to: h is an input, stated on
every output. It quantifies what conduction geometry can and cannot fix —
and at M4 it will say so on a receipt.
