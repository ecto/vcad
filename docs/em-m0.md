# Electromagnetics M0: solved fields, inductance, capacitance, force, torque

`vcad-kernel-em` makes vcad a design tool for the magnetics-and-fields
family of devices: inductors and coil pairs, actuators and magnet
assemblies, capacitive structures, and — the flagship — PCB motors. All of
them reduce to the same loop —

> geometry → fields → energy/force functionals → figure of merit

— and the incumbent tool for that loop (FEMM, 2004) is Windows-bound,
non-differentiable, and disconnected from CAD. vcad itself has so far
priced EM with formulas (`calc_coil`, `calc_motor`'s
`Kt = k_w·N·p·B·A_pole`, MEC reluctance networks). This crate is the M0 of
a differentiable, receipt-native replacement that solves the fields the
formulas approximate — next to the geometry that generates them.

## M0 scope (and honesty)

**In scope:** linear statics on 2D domains, three formulations on one
shared symmetric finite-volume core (`∇·(c∇u) = −s`, face conductances,
SOR with a Chebyshev relaxation estimate and scale-invariant stopping):

- **Axisymmetric magnetostatics** on the flux function ψ = r·A_θ
  (coils of revolution: solenoids, coaxial pairs; linear μ_r regions).
  Outputs: energy (two discrete forms + balance residual), flux linkage,
  self/mutual inductance — the energy and linkage routes agree
  *identically* by construction — axial force by J×B and independently by
  Maxwell stress on a closed cylinder.
- **Planar magnetostatics** on A_z (motor cross-sections; permanent
  magnets as bound-current edge sheets K = B_r/(μ₀μ_r); optional periodic
  x for unrolled rotating machines). Force/torque by J×B on an element's
  own deposits and by Maxwell stress on a full-period gap line or an
  air-gap circle.
- **Electrostatics** on φ, both geometries: capacitance by energy and by
  induced-charge flux, reported together (their gap is a quality
  diagnostic).

**Out of scope at M0** (each is a milestone below): saturation and B–H
curves, eddy currents / AC, the discrete adjoint, 3D. Also honestly
absent: curved material boundaries staircase at grid resolution (an O(h)
surface bias, measured below); ψ = 0 truncation boundaries squeeze
far-reaching return flux unless placed far (measured below); planar motor
slices ignore curvature and radial end effects; thin features floor at
the cell size.

Lessons inherited from `vcad-kernel-particle` and encoded from the start:
fields are sampled as the **exact derivative of the interpolant**
(divergence-free B by construction, conservative E; the sampler is
second-order at cell centers and carries a known h/(2r) staggering offset
on nodes); solver stopping is **scale-invariant** (a 1e−30-scale source
must converge in the same sweep count as a unit one — regression-tested);
every extracted quantity comes with an internal cross-route check.

## Validation ladder (all in `cargo test -p vcad-kernel-em`, ~5 s debug)

Exact anchors (discretization-limited, no modeling gap):

| rung | reference | result |
|---|---|---|
| infinite solenoid (Neumann far/symmetry BCs) | Ampère's law, incl. winding-thickness term | O(h²): rel. err 1.5e−2 → 6.0e−5 over h = 2 → 0.125 mm (×4.00/halving) |
| + μ_r = 50 core | H set by free currents alone | < 1% |
| sheet-pair transmission line | L′ = μ₀(gap + 2t/3)/w | < 0.5% |
| series dielectric capacitor | C′ = ε₀w/(d₁/ε₁ + d₂/ε₂) | < 1e−6 (interface on a node row: discretely exact) |
| coax (Neumann ends) | 2πε₀/ln(b/a) | < 0.2%, energy vs charge routes < 1e−6 apart |

Published closed forms and independent code:

| rung | reference | result |
|---|---|---|
| loop B field, 6 probes | `vcad_kernel_particle::field::b_ring` — an independent elliptic-integral implementation in this workspace | 0.8–3.9% at h = 1.25 mm (budget grows with probe distance; see truncation rung) |
| finite solenoid L | Wheeler 1928 (±1% claimed) | **0.15%** |
| coaxial-loop mutual M | Maxwell's elliptic formula (Smythe §8.06) | 3.0% (1 mm² sections vs filaments, h = 1 mm) |
| coaxial-coil force | F = I₁I₂·dM/dz | J×B **0.08%**; Maxwell-stress surface vs J×B 2.6%; action–reaction < 0.1% |
| concentric spheres C | 4πε₀ab/(b−a) | staircase O(h): 5.0% → 2.1% over n = 41 → 121, monotone |
| wire in uniform B | F = I·B | J×B and stress circle both < 3%, torque ≈ 0 |

Failures that taught, kept as regression tests:

- **ψ = 0 truncation**: a 4R domain read the far-axis loop field 5.6% low.
  The dedicated rung re-measures the same point at two domain sizes at
  fixed h and asserts the error shrinks — truncation, proven, not assumed.
- **Sheet-pair boundary conditions**: fixing A = 0 on *both* sides of a
  current-sheet pair forces a spurious return flux (the pair carries a net
  A jump); one Dirichlet + one Neumann side is the ideal-line condition.
- **Wheeler is a current sheet**: a 1 mm-thick winding's inner-weighted
  linkage reads a real 3% below the sheet formula — the solver was right,
  the comparison was wrong. The rung models the sheet.
- **Material sampling ties**: point-sampled μ at face midpoints let a
  region edge on a node line resolve by float dust — it silently broke
  mirror symmetry (2% action–reaction error). Materials now live on
  cells, sampled at centers, faces take the parallel sum of flanking
  half-cells; a mirror-symmetric magnet pair must cancel forces to < 0.1%.
- **Probe conditioning**: comparisons near the loop's dipole zero cone
  (θ ≈ 54.7°) are ill-conditioned by construction; probes sample where
  the reference is O(|B|), at cell centers computed from the grid.

## Benchmark: the 70 mm PCB motor, from its real copper

`cargo run --release -p vcad-kernel-em --example motor_torque`

The fabricated design of `examples/pcb-motor` (9s6p, 10 turns/tooth-coil,
spiral radii 2.6–7.2 mm, Y30 ferrite Ø15×3 discs, 1.0 mm gap, 2.7 mm
steel irons) as an unrolled periodic slice at the 22.5 mm pitch radius,
560×81 cells, energy balance 5e−7:

- **Air-gap flux under a pole center: 0.201 T solved** vs the repo MEC's
  0.204 T raw — the reluctance network's raw number is confirmed by a
  field solution to 1.5%.
- Torque is linear in current (linear materials — the fit is exact), and
  the two extraction routes (full-period Maxwell stress line, J×B on the
  magnets' bound sheets) agree to 1.6% at every current.
- **Kt = 3.13 mN·m/A** (peak sinusoidal phase current, best commutation
  angle) vs the shipped design's verified 3.70 (formula at derated MEC
  flux) and 5.31 (formula at solved B).

**Finding — the spiral pitch factor.** The 0.866 in the first-order
formula is the 9s6p *slot* factor, which assumes each coil spans a full
tooth pitch (120°e). The as-built spiral spreads its radial conductors
2.6–7.2 mm off the tooth axis — its turns span 40–110°e, and averaging
sin(half-span) over the spread gives an honest winding factor of
**0.598**. Substituting it brackets the field solve (2.83…3.67 vs 3.13
solved, derated↔solved flux conventions): the field solution and the
harmonic analysis agree, and the shipped Kt estimate is ~15–20%
optimistic. Design lever exposed: push spiral copper outward (fatter
outer turns, hollow center) to widen the effective span. The 2D slice
itself still ignores curvature and radial end fringing — its Kt is an
estimate with stated omissions, not a measurement (that's M6's job).

## Milestone ladder

- **M0 — solver + extraction + validation ladder. DONE** (this
  document). Shared symmetric FV core; axisym + planar magnetostatics;
  electrostatics; inductance/capacitance/force/torque each by two routes;
  the ladder above; the motor benchmark.
- **M1 — nonlinear μ (B–H) + AC phasor eddy currents. DONE.**
  Saturation via the arctangent B–H law (initial slope μ₀μ_ri, correct
  deep-saturation slope μ₀), iterated per cell by Picard **damped on the
  solved H** — two schemes failed first and are documented in the code:
  damping ν has map derivative ~μ_r (measured 64×/iteration swing);
  damping B explodes on the low-B branch of MMF-driven cores (slope
  H/H_curve ≈ 50). The H update is exact in one step when Ampère pins H.
  Validated against an **exact nonlinear anchor**: the Neumann-bounded
  infinite solenoid, where flux linkage is `N·B_curve(n·I)·πR²` in
  closed form — held to <1% from the linear region through the knee into
  deep saturation, plus linear-limit equivalence and monotone secant-L
  droop. Phasor eddy currents (`−jωσ·A` diagonal on the same stencil,
  complex Gauss–Seidel — the Chebyshev-optimal ω is *defective* and even
  a 1.4% imaginary diagonal makes it diverge, hence ω = 1 guaranteed by
  diagonal dominance, and NaN now fails closed in both solvers):
  analytic slab skin depth (magnitude e⁻¹ and 1 rad per δ, <3%),
  field-vs-circuit eddy-loss agreement to 2 ppm, `L_ac → L_dc` and
  `R_eddy ∝ ω²` low-frequency limits, and the conducting rod at R/δ = 2
  against the complex-Bessel closed form `2J₁(kR)/(kR·J₀(kR))` (<2%,
  amplitude and phase). Honesty: single-valued isotropic curve (no
  hysteresis), phasor solve is linear-materials-only (saturation + AC =
  harmonic balance, out of scope), source windings carry no internal
  skin effect.
- **M2 — discrete adjoint. DONE** (`adjoint.rs`). One adjoint solve per
  quantity of interest on the same symmetric operator (`A·λ = ∂J/∂u`,
  homogeneous Dirichlet), then every parameter is priced:
  `dJ/dI_k = λᵀU_k` (+ explicit terms), `dJ/d(Br scale)` per magnet, and
  `dJ/dG_f = −Δu_f·Δλ_f` rolled to `dJ/dμ_r` per region through **face ←
  cell incidence weights recorded by the builders themselves** — the
  adjoint differentiates the exact assembly the solver used (a
  bit-exact reconstruction test guards drift). QoIs shipped: axisym flux
  linkage (whose current-gradient must and does reproduce the
  inductance matrix to 1e−6) and coil force; planar rotor torque about a
  point (the motor objective) with conductor-current, magnet-strength,
  and iron-μ gradients. All FD-validated at frozen discretization;
  linear-in-I gradients agree to solver tolerance. Fail-closed honesty:
  material gradients of *saturable* regions are refused (the secant ν
  depends on the field through the Picard fixed point — FD through
  `solve_nonlinear` is the honest route until the fixed-point adjoint
  lands); geometry gradients stay FD (deposit masks are discrete), the
  same role split as the particle crate. A test lesson kept: gradient
  comparisons must sit on quantities of honest size — a mirror-symmetric
  test machine had torque ≈ 0 and both routes compared solver dust.
- **M3 — parameter seam. DONE** (`spec.rs`). `AxisymSpec` /
  `PlanarSpec`: every numeric field literal-or-named, fail-closed
  resolution (unbound name = error, never a default),
  `parameter_roles()` classifying by what the adjoint *actually
  implements* — currents, magnet remanence, and linear-material μ are
  `Adjoint`; geometry, turns, magnet recoil, and saturable-material μ
  are `FiniteDifference`; a name shared across roles demotes
  conservatively. JSON round-trip incl. saturation and Neumann BCs.
  BRep extraction stays on the vcad side of the seam.
- **M4 — receipt claims. DONE** (`receipt.rs`, `vcad.em-claims/1`).
  inductance_h, stored_energy_j, force_n, torque_nm, capacitance_f —
  every claim with formulation/grid/tolerance/sweeps provenance,
  spelled-out caveats, and the crate's signature
  **`cross_route_residual`**: the two-independent-routes gap (energy vs
  linkage, charge vs energy, Maxwell stress vs J×B) carried on the
  receipt itself. Fail-closed `compare()`: Holds/Violated/Unmeasured,
  an unmeasured receipt never passes, a measurement matching no claim
  is an error. Registering the family in `crates/vcad-receipt` + MCP
  tools is the flagged cross-crate follow-up PR.
- **M5 — published benchmark + convergence + paper draft. DONE.**
  Benchmark chosen for stateable-exactly published truth (no
  from-memory "TEAM numbers"): the permeable cylindrical shell in a
  uniform transverse field, Jackson 3rd ed. problem 5.14
  (`H_in/H₀ = 4μb²/((μ+1)²b²−(μ−1)²a²)` = 0.10182 for a = 8 mm,
  b = 10 mm, μ_r = 100) — a composite of 100× μ jumps, curved
  staircased interfaces, and a uniform applied field, solved with the
  new `RingMaterial` planar region. `examples/convergence.rs` prints
  the two-regime table: the smooth solenoid anchor at measured order
  2.00/2.00/1.99/1.95, the staircased shell at 32.7% → 5.1% → 1.7%
  (h = 1 → 0.25 mm, ~O(h)) — and at h = 2 mm the 2 mm shell is one
  cell thick and the answer is garbage: the thin-feature floor,
  demonstrated numerically. Regression rung at h = 0.67 mm.
  `docs/em-paper-draft.md` carries the full ladder and the motor case
  study with real numbers only.
- **M6 — measurement pack. DONE.** `examples/motor_receipt.rs` emits
  the motor's `vcad.em-claims/1` JSON (torque 4.64 mN·m @ 1.5 A, 1.6%
  cross-route residual on the receipt) and demonstrates the fail-closed
  binding; `docs/em-measurement-pack.md` is the bench procedure:
  Kt via back-EMF (drill + scope, speed from the waveform's own
  electrical frequency — expected 0.57 V line-line peak at 1000 RPM),
  stall-torque cross-check on a jewelry scale, Hall-probe air-gap flux
  vs the 0.201 T claim, spin-down friction context, and an honest
  no-prediction list (phase L and R: end turns unmodeled — binding
  them would manufacture a Violated verdict the model already
  declares). Bands sized by the slice's stated omissions.

## Non-goals

This crate does not claim FEM-grade geometry fidelity at M0 — curved
boundaries staircase and the convergence study is part of every claim. It
replaces *formulas* with *fields* and prices what the formulas hide; it
says what it does not model on every output.
