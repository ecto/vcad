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

## Milestone ladder (all landed on this branch)

- **M0 (done)** — isothermal laminar internal flow; Poiseuille, Shah–
  London, Ghia; `vcad.flow-claims/1`; splitter-manifold flagship.
- **M0.5 (done)** — WASM bindings + `simulate_flow` MCP tool (spec JSON in,
  claim set out), kernel-features catalog entry, changelog.
- **M1 (done)** — thermal transport in the fluid as a **second D3Q19
  distribution** carrying θ = T − T_inlet (bounce-back = adiabatic,
  anti-bounce-back = Dirichlet, copy-own = outflow). The plan left the
  scheme choice to in-branch testing; the FV donor-cell route was tried
  and **rejected** — its discrete divergence differs from the lattice's
  and it violated the maximum principle near the inlet. New claims:
  `outlet_temp_c`, `heat_pickup_w`, and a two-route
  `thermal_energy_residual` (field route: outlet enthalpy flux from the
  velocity/temperature fields; link route: the θ the scalar lattice
  actually exchanged at its Dirichlet boundaries — < 5% at 7 voxels
  across).
- **M2 (done)** — the conjugate seam (`conjugate::solve_conjugate`):
  segregated loop — flow prices a film h from the wall heat its lattice
  actually moved (correlation-seeded on the bootstrap iteration) plus a
  bulk temperature into thermal's `Boundary::Convection`; thermal hands
  its surface temperature field back into `FlowModel::solid_temp_c`;
  iterate to a wall-temperature fixed point, fail-closed on budget.
  **Film-averaged** (thermal's `exposed` slot is single) — the per-voxel
  direction is wall temperatures only; noted where the plan had hoped
  for per-voxel h. Validation: heated-block-under-duct closes the
  energy loop (fluid pickup = source power within 10%). The thermistor
  measurement pack rides `receipt::compare` as designed.
- **M3 (done)** — natural convection: Boussinesq per-cell forcing,
  buoyancy counts as drive, fail-closed Ra ≤ 10⁸ envelope, per-voxel
  `solid_temp_c` painting for differential heating. Validation: de Vahl
  Davis Ra = 10³ cavity, mean hot-wall Nu = 1.118 within 0.08 on 33²
  (release rung).
- **M4 (done)** — discrete adjoint via the **reverse fixed point** of
  one lattice step (`λ ← CᵀSᵀλ + ∂J/∂f` — a steady state needs no
  checkpointing, improving on the plan's checkpointed sketch), Brinkman
  drag parameterization, closed-form collision transpose. Gates: ε = 0
  agrees with the plain solver to 1e-6; adjoint vs central FD < 1%.
  Found and fixed en route: freezing the pressure-outlet's quadratic
  velocity term destabilizes the transpose — the u-coupling damps the
  anti-bounce-back −1 self-link, so the transpose carries it.
  `optimize_channel` (projected gradient, bisected volume multiplier)
  beats a uniform 40%-solid start by > 10% ΔP at equal solid fraction.
- **M5 (done, GPU + viewport plumbing)** — `gpu` feature: the same
  D3Q19 step as WGSL compute (`gpu::preview_blocking/preview_async`) on
  `vcad-kernel-gpu`'s shared context, browser-capable. Explicitly the
  **preview lattice**: f32, fixed steps, no steadiness detection, every
  result carries a not-claim-grade note; the CPU↔GPU parity test pins
  the preview to the claim-grade solver at steady state (< 2e-3
  relative, skips cleanly on adapterless machines). App: `FieldKind`
  gains `velocity`/`pressure` for the existing per-vertex overlay path.
  The volumetric smoke/streamline renderer is deliberately its own app
  PR (new render machinery, the demo-video deliverable).

## Non-goals

Turbulence models (RANS/LES), compressible or transonic flow, multiphase
and free surfaces, non-Newtonian fluids, external aerodynamics, porous
media. A shared mesh→voxel occupancy utility across thermal/topopt/flow
is deliberately deferred — each solver currently owns its grid, and the
conjugate seam only needs a shared occupancy *vector*, not a shared
voxelizer.
