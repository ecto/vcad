# Flow M0: laminar internal-flow CFD with a lumped-oracle conscience

`vcad-kernel-flow` closes the last uncovered rung on the simulation scale
ladder: things that flow. It answers *how much flow do I get, and what
pressure does it cost?* with two independent routes to the same numbers —
a D3Q19 lattice-Boltzmann field solver and closed-form duct-loss
correlations — and refuses, fail-closed, everything it has not validated.

The name is `flow`, not `cfd`, on purpose: the lumped correlations are a
first-class feature (instant sizing answers at design time), not a
fallback. The receipt carries the gap between the two routes as
`cross_route_residual`, the same convention as `vcad-kernel-em`.

## M0 scope (and honesty)

Single-phase, incompressible, isothermal, **laminar** internal flow on a
uniform cubic voxel grid. Geometry arrives painted (`spec::FlowSpec`,
boxes and tubes, painter's order) or as an externally-voxelized occupancy
vector — the same grid convention as `vcad-kernel-thermal`, deliberately,
so the M2 conjugate solve shares one grid.

Where the numbers are optimistic or refused, stated up front:

- **No turbulence model.** The model validation computes the inlet
  Reynolds number (`Re = ρ·|U|·D_h/μ`, patch hydraulic diameter) and
  refuses above `re_envelope` (≤ 2300, the pipe-flow transition; the
  envelope can be lowered per model, never raised). A converged-looking
  laminar solve at Re 10⁴ would be a wrong answer wearing a receipt.
- **BGK stability is a second, sharper gate.** The unit scaling derives
  the relaxation time τ from viscosity, voxel size, and reference speed,
  and refuses outside the validated window τ ∈ [0.52, 1.95]. The floor is
  a cell-Reynolds stability bound observed in the ladder (BGK at τ ≈ 0.51
  diverges from developed-profile peaks); the error names the refinement
  factor that fixes it. In practice this gate binds before the Re
  envelope: air at 0.2 m/s on a 1 mm grid is refused with "refine ~2×",
  which is the truth. MRT collision would relax the floor — a flagged
  option for a later milestone, not an M0 knob.
- **Weak compressibility.** LBM pressure fields carry acoustic noise of
  order Ma²; the scaling caps the lattice Mach at 0.087 (u_lat ≤ 0.05),
  i.e. ≤ 0.75% pressure noise, and each claim's note states the actual
  Ma² of the run.
- **Voxel staircases.** Walls are axis-aligned voxel faces (half-way
  bounce-back, second-order for aligned walls). Inclined or curved
  channels resolve at first order in staircase width — refine, and watch
  the convergence study, before believing tight tolerances on curved
  geometry.
- **Inlet plug flow is a fiction at the edges.** The moving-wall inlet
  injects the link-realized flux, which is less than plug `U·A` where the
  patch touches walls (no-slip eats the edge links). `Solution` reports
  the realized flux; the deficit is physical, not lost mass.
- **Steadiness is detected, never assumed.** The velocity field's
  relative L∞ change per check interval must fall below tolerance; a run
  that exhausts its step budget is an error carrying the residual, not a
  result. Impulsive starts are smoothed by a smoothstep inlet ramp.

## Validation ladder (all in `cargo test -p vcad-kernel-flow`)

1. **Lattice identities** — weights sum to 1, first/second moments
   recover c_s² = 1/3, equilibrium conserves mass and momentum to 1e-12.
2. **Poiseuille channel** (body-force-driven, periodic) — L2 profile
   error against the exact parabola **< 1%** at 21 cells across; walls
   land exactly half-way outside the last cell, so a channel n cells wide
   is exactly n·dx wide.
3. **Grid convergence** — the same case at 11 vs 21 cells shows the
   error falling ≥ 2× (second-order stencil with a BGK slip term).
4. **Square duct vs Shah–London** — the exact rectangular-duct series
   solution reproduces f·Re = 56.91 to 0.2%; the LBM developed-core
   pressure gradient agrees with that oracle to **< 8%** at 9 voxels
   across (the test asserts its own precondition: entrance length before
   the measurement window).
5. **Lid-driven cavity vs Ghia et al. (1982), Re = 100** — vertical-
   centerline u profile within 0.05 of the 129² reference on a 33² grid
   (release-ladder rung: `cargo test --release -p vcad-kernel-flow --
   --ignored`).
6. **Mass audit** — every ported solve reports
   `|Q_in − Q_out|/max(|Q_in|,|Q_out|)`; the duct rung requires < 1%.
7. **Refusals** — Re above envelope, τ outside window, unbalanced ports,
   no drive, non-cubic voxels, unconverged budget: all errors, all
   naming the fix.

## The flagship: the splitter manifold

`examples/manifold.rs` — a printed 4 mm splitter manifold (one inlet
tees into two outlet ducts, 80×32×12 grid at 0.5 mm). Steady in ~5k
steps; the symmetric split comes out 0.500/0.500, mass residual 4e-5,
and the claim set prints with full provenance: τ, Ma, Re and envelope,
steps, steadiness residual, and the cross-route residual against the
straight-duct oracle. This is the shape of every future flow receipt:
two routes, one gap, no unexplained numbers.

## Claims: `vcad.flow-claims/1`

`receipt::predicted_claims` emits `pressure_drop_pa`, `flow_rate_m3_s`,
`mass_balance_residual`, `max_speed_m_s` — every claim
`basis: "predicted"`, every note stating the missing physics and the h…
er, the Ma², staircase, and envelope caveats. `design_claims` adapts the
family onto the unified `vcad.receipt/1` schema (domain `flow`), where a
receipt built from predictions **rolls up Provisional, never Pass**.
`compare` binds bench measurements (flow loop, micromanometer) with
fail-closed verdicts — Unmeasured never silently passes. The printed
measurement pack that closes the loop is M2.

## Milestone ladder

- **M0 (this)** — isothermal laminar internal flow; Poiseuille, Shah–
  London, Ghia; `vcad.flow-claims/1`; splitter-manifold flagship.
- **M0.5 (done)** — WASM bindings + `simulate_flow` MCP tool (spec JSON in,
  claim set out), kernel-features catalog entry, changelog.
- **M1** — thermal transport in the fluid (advection–diffusion on the
  LBM field, fixed wall temperatures); Nu-correlation oracles join the
  cross-route check; `outlet_temp_c`, `heat_transfer_w` claims.
- **M2** — the conjugate seam to `solve_thermal`: segregated exchange —
  flow hands per-boundary-voxel film h and T_fluid into thermal's
  `Boundary::Convection`, thermal hands wall temperatures back; iterate
  to interface heat-flux convergence, fail-closed on the iteration
  budget. Fan-cooled enclosure flagship ("does this stay under 60 °C"),
  plus the measurement pack (thermistor + printed orifice) that flips
  Provisional → Pass.
- **M3** — natural convection (Boussinesq forcing, Ra-gated laminar
  envelope, de Vahl Davis cavity benchmark): the fanless-enclosure
  answer.
- **M4** — discrete adjoint of the steady state (checkpointed reverse
  mode, FD-validated) → flow-channel topology optimization: grown
  manifolds and heatsinks with receipts on both ΔP and temperature.
- **GPU track (parallel, any time after M0)** — LBM streaming/collision
  as WGSL compute on `vcad-kernel-gpu`; CPU↔GPU parity tests gate
  claim-grade use; until then GPU output is preview-only ("preview
  lattice") — the live smoke-in-viewport demo rides here.

## Non-goals

Turbulence models (RANS/LES), compressible or transonic flow, multiphase
and free surfaces, non-Newtonian fluids, external aerodynamics, porous
media. A shared mesh→voxel occupancy utility across thermal/topopt/flow
is deliberately deferred — each solver currently owns its grid, and the
conjugate seam only needs a shared occupancy *vector*, not a shared
voxelizer.
