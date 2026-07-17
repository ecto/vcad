# Gradient-based electrode design for electrostatic confinement devices (draft)

**Status: internal draft toward an arXiv submission (physics.plasm-ph /
physics.acc-ph). Numbers below are reproducible from
`crates/vcad-kernel-particle` at the stated commands; the SIMION
cross-validation section is planned, not done.**

## Abstract (draft)

Electrode geometry for electrostatic and magneto-electrostatic confinement
devices — fusors, magnetically shielded grids, ion traps, thruster grids —
is still designed by hand, guided by ray-tracing tools that are decades
old, non-differentiable, and disconnected from manufacturable CAD. We
present a differentiable design pipeline for axisymmetric devices: a
finite-difference Laplace solver whose sampled field is the exact gradient
of its interpolant (making integrator energy error purely a time-step
property), a Boris tracer with fusion-yield weighting via the Bosch–Hale
D-D cross sections, figures of merit aligned with bench observables
(cathode interception current, neutron rate), and a discrete adjoint that
delivers exact-to-discretization gradients of traced yield with respect to
every electrode potential and coil current at the cost of roughly one
extra field solve. We reproduce the magnetically-shielded-grid effect
[Hedditch et al. 2015] from first principles, recover the r_L ∝ √V
commissioning law, quantify a non-obvious optimum in shield current
(beyond it, the cusp reflects ions off the fusion core itself), and show
the design landscape is multimodal — single-start gradient ascent stalls
on a recirculation hill at 3× lower yield than the energy-quality basin a
multi-start finds. We argue (and measure) that on long horizons the traced
objective is chaotically rough, where finite differences under-read
gradients by 3–4× — precisely the regime where the adjoint is the only
trustworthy gradient. Every prediction is emitted as a machine-readable
claim with solver provenance, including a distance-to-Lawson figure that
prices the device against breakeven honestly.

## 1. Motivation

- The electrode-design loop (geometry → fields → trajectories → figure of
  merit) is common to IEC fusion research devices, ion thrusters,
  quadrupole/orbitrap mass spectrometry, electron optics, and ion traps.
- The incumbent tooling lineage (SIMION et al.) is a forward ray tracer:
  no gradients, no fabrication output, no provenance.
- IEC specifically: the 2015 magnetically-shielded-grid proposal
  (arXiv:1510.01788) remains unbuilt and largely unsimulated in public;
  the amateur record (~5×10⁶ n/s D-D) has stood with hand-designed
  cathodes.

## 2. Methods

### 2.1 Fields
Axisymmetric Laplace (vacuum) on a uniform (r, z) node grid; SOR with the
Chebyshev-estimate relaxation factor; wire-ring electrodes and the chamber
as Dirichlet masks (mask radius floored at 0.75 grid cells — see the
convergence study). **Conservative sampling:** E is the analytic gradient
of the bilinear potential patch, not an interpolation of node-difference
fields, so the sampled field's line integral between any two points equals
the interpolant's potential difference; integrator energy error is then
governed by the time step alone. Coil fields are exact single-loop
solutions (complete elliptic integrals via AGM), superposed, grid-cached
away from conductors and analytic within 8 wire radii so the shielding
sheath keeps its 1/ρ structure.

### 2.2 Tracing and figures of merit
Boris pusher; adaptive step capped by grid spacing and refined within 6
wire radii; gyration substepping. Termination on wire/wall impact;
core-pass counting; per-trace far-field energy-drift diagnostic. Fusion
weighting: ∫σv dt along each trace for both D-D branches (Bosch & Hale
1992), reported per ion against a stated background density. Optional
charge-exchange model (constant σ_cx): survival-weighted ion channel plus
a straight-line fast-neutral channel; the CX chain (each event births a
re-accelerated cold ion) is explicitly not modeled and stated as such.

### 2.3 Discrete adjoint
Reverse-mode differentiation of the ensemble yield: fixed-step
self-consistent forward (adaptive dt is non-differentiated control flow;
a pure time budget removes termination boundary terms), reverse Boris
chain with PIC-style deposits into the potential grid through the exact
patch weights, then one adjoint Poisson solve using the radial-weight
symmetrization of the axisymmetric operator (w_i = r_i; axis row Δr/8).
Coil-current gradients via ⟨λ_B, B(I=1)⟩. Validated against central
differences end-to-end: dJ/dI to 0.1%, dJ/dV to 0.8%, and a gauge test
(Σ over all conductors of dJ/dV ≈ 0). Shape parameters remain finite-
difference (the Dirichlet mask is discrete); smooth shape adjoints via
boundary-value differentiation are future work.

### 2.4 Validation ladder
Elliptic integrals vs tabulated values; loop field vs the on-axis closed
form and midplane symmetries; Poisson maximum principle, axis symmetry,
refinement consistency; Boris energy conservation; magnetic-mirror loss
cone (pitch-angle dichotomy at mirror ratio ≈ 3, including the observed
non-adiabatic punch-through of fast ions — r_L comparable to the field
scale — which the criterion does not protect); axial oscillation period
vs well curvature (2π/ω from the solved potential, ±20% with stated
anharmonicity); Bosch–Hale anchors.

## 3. Results (current numbers)

All from `cargo run --release -p vcad-kernel-particle --example …`.

**Shielded-grid effect and √V law** (`fusor_baseline`): at fixed geometry
and −3 kV bias, wire interception falls 1.00 → 0.12 by 160 kA·turns; the
−30 kV interception curve is the −3 kV curve shifted ~3–4× in current,
matching r_L ∝ √V. Commissioning hardware cold with pulsed copper before
climbing voltage is thereby quantitatively supported.

**Yield optimum and multimodality** (`optimize_shield`): σ(E)-weighted
yield at −30 kV rises 6× by 160 kA·t. The optimizer finds a local
recirculation hill at ≈26 kA·t (2.0× unshielded) that gradient ascent
cannot leave; multi-start reaches the energy-quality basin at ≈165 kA·t
(6.2× unshielded) with ring spacing driven to its box bound — the machine
telling us the hand-chosen parameterization was too narrow. Beyond the
optimum, mean core passes fall while interception keeps dropping: the
cusp begins reflecting ions off the core (magnetic aperture), a tradeoff
we have not found stated quantitatively for this configuration.

**Fusor floor** (`fusor_baseline`): a classic 5-ring fusor at 30 kV,
10 mA, 2 mTorr predicts 1.9×10⁵ n/s from the beam-on-background channel —
a first-principles floor ~25× under the amateur DIY record, as it should
be given the unmodeled fast-neutral chain. With single-generation charge
exchange enabled, the surviving-ion channel collapses (CX mean free path
< one pass at 2 mTorr) and totals land at 1.9×10⁴ n/s; the gap to
measured fusors prices the CX chain + volume ionization at roughly 30×,
and yield becomes nearly pressure-independent — matching fusor lore.

**Gradient trustworthiness**: on smooth short horizons FD converges to
the adjoint (0.1–0.8%); on long horizons the traced objective is
chaotically rough and FD at practical steps under-reads by 3–4×,
non-monotonically in h. Design loops that rely on FD there are optimizing
noise.

**Convergence** (`convergence`, classic fusor, 1 mm wire, 48 ions,
20-pass cap):

| grid | intercept | mean passes | σv (m³) | mean drift |
|---|---|---|---|---|
| 61×121 | 1.000 | 4.4 | 1.44e−32 | 0.081 |
| 81×161 | 0.896 | 7.4 | 2.15e−32 | 0.089 |
| 121×241 | 0.771 | 7.9 | 2.17e−32 | 0.055 |
| 161×321 | 0.604 | 10.2 | 3.23e−32 | 0.051 |

Interception tracks the mask floor (0.75·h inflates a 1 mm wire until
h ≲ 1.3 mm): thin-wire configurations demand h ≲ a, and headline sweeps
(3 mm wire at 121×241) sit comfortably inside that; 1 mm-wire absolute
yields carry O(30%) discretization and are quoted as trends only.

**Receipts**: every configuration emits `vcad.particle-claims/1` — the
classic fusor card reads Q = 4.1×10⁻¹⁰, distance-to-Lawson 9.39 orders,
with grid, tolerance, ensemble, censoring, and channel provenance
attached to each claim.

## 4. Limitations and roadmap

No space charge, no collisions beyond the constant-σ CX approximation, no
CX chain, single-species, axisymmetric only, shape gradients by FD. Each
is a milestone in `docs/particle-optics-m0.md`. SIMION cross-validation
on published einzel/trap geometries is the outstanding external
comparison before submission.

## References (to be completed)

- J. Hedditch, R. Bowden-Reid, J. Khachan, "Fusion in a magnetically-
  shielded-grid inertial electrostatic confinement device", Phys. Plasmas
  22, 102705 (2015); arXiv:1510.01788.
- H.-S. Bosch, G.M. Hale, "Improved formulas for fusion cross-sections
  and thermal reactivities", Nucl. Fusion 32, 611 (1992).
- T.H. Rider, "Fundamental limitations on plasma fusion systems not in
  thermodynamic equilibrium", PhD thesis, MIT (1995).
- J.P. Boris, "Relativistic plasma simulation — optimization of a hybrid
  code", Proc. 4th Conf. Numerical Simulation of Plasmas (1970).
- S.E. Wurzel, S.C. Hsu, "Progress toward fusion energy breakeven and
  gain as measured against the Lawson criterion" (arXiv:2505.03834).
