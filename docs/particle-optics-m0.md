# Charged-particle optics M0: vacuum fields, Boris tracing, electrode figures of merit

`vcad-kernel-particle` makes vcad a design tool for the electrode-geometry
family of devices: fusors and magnetically shielded IEC machines, ion traps,
ion-thruster grids, mass-spec optics, electron guns, X-ray tubes, ion
implanters. All of them reduce to the same loop —

> electrode geometry → fields → charged-particle trajectories → figure of merit

— and the incumbent tool for that loop (SIMION lineage) is decades old,
non-differentiable, and disconnected from CAD/fabrication. This crate is the
M0 of a differentiable, receipt-native replacement that lives inside the
kernel next to the geometry it scores.

## M0 scope (and honesty)

**In scope:** vacuum-field single-particle optics.

- Axisymmetric electrostatics: finite-difference Laplace on the (r, z)
  chamber cross-section, SOR with the Chebyshev-estimate relaxation factor,
  Dirichlet electrodes (wire rings + grounded chamber), symmetry stencil on
  the axis. No space charge.
- Magnetics: exact off-axis field of circular current loops via complete
  elliptic integrals (AGM), superposed per ring; linear regularization
  inside the conductor.
- Tracing: Boris pusher (the plasma-standard geometric integrator), adaptive
  step from grid spacing and speed, automatic substepping where gyration is
  fast, termination on wire/wall impact, core-pass counting, per-trace
  energy-drift diagnostic.
- Figures of merit: interception fraction (the cathode ammeter),
  mean core passes (recirculation), effective transparency, wall losses,
  plus thin-wire geometric transparency as the no-optics reference.
- Optimization: box-constrained finite-difference gradient ascent
  (`optimize::maximize`) — the M0 stand-in for the adjoint, deliberately
  API-shaped so the adjoint can replace the gradient without changing
  callers.

**Out of scope at M0** (each is a milestone below): space charge,
collisions/charge exchange with neutrals, fusion-rate weighting along
trajectories, non-axisymmetric electrodes, the discrete adjoint.

**Regime of validity:** low-current, low-pressure devices where geometry
dominates — exactly the regime of electrode design tools. Do not read
plasma-physics conclusions (thermalization, virtual cathodes, space-charge
limits) out of these traces.

## Validation ladder (all in `cargo test -p vcad-kernel-particle`)

- Elliptic integrals against Abramowitz & Stegun values.
- Loop field against the textbook on-axis formula; odd/even midplane
  symmetry; finite and continuous through the conductor surface.
- Poisson: maximum principle, axis symmetry, grid-refinement consistency,
  well depth.
- Tracing: ions launched at rest fall inward and recirculate; energy
  conservation in the far field; wires dominate the fate table in a classic
  fusor.
- Headline integration test (`tests/shielding.rs`): at fixed geometry and
  bias, cathode ring current **reduces interception and increases
  recirculation** — the magnetically-shielded-grid effect
  (Hedditch/Bowden-Reid/Khachan 2015, arXiv:1510.01788), reproduced from
  first principles.

## M0 benchmark: the simulated ammeter

`cargo run --release -p vcad-kernel-particle --example fusor_baseline`

96 deuterons/config launched at rest from an 85% shell, 40-pass cap,
121×241 grid. Classic 5-ring fusor control: 9.65 mean passes, 100% eventual
wire interception, effective transparency 0.906 vs 0.925 geometric — the
lensing penalty, visible in a simulation for the first time in this repo.

Two-ring shielded cathode (45 mm rings at z = ±25 mm, 3 mm wire, opposed
currents), interception fraction vs ampere-turns:

| A·turns | −3 kV bias | −30 kV bias |
|---:|---:|---:|
| 0 | 1.000 | 1.000 |
| 5 k | 1.000 | 1.000 |
| 10 k | 0.958 | 1.000 |
| 20 k | 0.792 | 1.000 |
| 40 k | 0.365 | 0.854 |
| 80 k | 0.260 | 0.688 |
| 160 k | 0.073 | 0.406 |

Findings the sweep hands us for free:

1. **Shielding works and is enormous** — interception falls 100% → 7% at
   −3 kV / 160 kA·t; recirculation peaks at ~21 mean passes (5× the
   unshielded cathode).
2. **The √V law falls out.** The −30 kV curve is the −3 kV curve shifted
   ~3–4× right in current, matching r_L ∝ √V. Commissioning a real device
   at low voltage with pulsed copper, then climbing, is quantitatively
   supported.
3. **There is an optimal shield current.** Past it, mean passes *falls*
   while interception keeps dropping: the cusp begins reflecting ions away
   from the core itself (magnetic aperture). Shield strength trades
   transparency against core access — a real, non-obvious optimization
   target for the gradient loop. (Note: the 40-pass censor also biases the
   high-current mean down; both effects are real, disentangling them is an
   M1 diagnostic task.)

Known M0 limitation: worst-case per-trace energy drift reaches ~0.5·qΔV for
the extreme long-lived trajectories that repeatedly graze wire masks at high
B — the interpolated E field near a masked wire is under-resolved. Mean
behavior is unaffected (drift is diagnosed per trace, and the classic-fusor
acceptance test holds it < 8%), but M1 should add local grid refinement or
an analytic near-wire E model before quantitative loss budgets are claimed.

## Milestone ladder

- **M1 — near-field fidelity + loss budget.** Mask-aware near-wire E
  (analytic wire-in-external-field patch or local refinement), censoring
  diagnostics, fusion-rate weighting along trajectories (beam–background
  σ(E) integral) so runs report relative fusion yield, not just passes.
- **M2 — discrete adjoint.** Reverse-mode differentiation of the Boris loop
  (checkpointed; Boris is symplectic and its reverse pass is clean) and of
  the bilinear field sampler; adjoint of SOR via the adjoint Poisson solve
  (self-adjoint operator — one extra solve per objective). Replaces
  `optimize::maximize`'s FD gradient behind the same API; wires into the
  existing `vcad-kernel-diff` L-BFGS.
- **M3 — geometry seam.** Electrode cross-sections from vcad sketches /
  revolved BRep sections instead of hand-parameterized rings; parameters
  become named `.vcad` document parameters (same contract as
  `document_parameter_gradient`).
- **M4 — receipts + MCP.** `simulate_charged_particles` /
  `optimize_electrodes` tools; claims land in the DesignReceipt as a new
  claim family (interception fraction, recirculation, transparency, with
  solver provenance: grid, tolerances, ensemble size, censoring). This is
  also where `distance_to_lawson` gets its trajectory-level inputs.
- **M5 — validation against the incumbent.** SIMION cross-checks on
  published einzel/trap geometries; convergence study; the arXiv writeup:
  *gradient-based electrode shape optimization for electrostatic
  confinement devices*, with the shielded-grid sweep as the flagship
  result.
- **M6 — experiment pack.** The shielded-grid experiment BOM (chamber,
  feedthrough, pulsed ring supply, cathode ammeter), predicted-vs-measured
  receipt schema: the sim predicts the ammeter curve, the bench measures
  it, the receipt binds them.

## Non-goals

This crate does not claim a path to net energy gain. It quantifies,
per-geometry, the loss channels that electrode design can and cannot
close — and at M4 it will say so on a receipt.
