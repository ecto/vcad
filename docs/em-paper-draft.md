# A differentiable, receipt-native 2D electromagnetic field solver inside a CAD kernel

*Draft — all numbers current as of the M5 milestone of `vcad-kernel-em`;
regenerate with `cargo test -p vcad-kernel-em` and the `convergence`,
`ladder_numbers`, and `motor_torque` examples.*

## Abstract

Formula-grade electromagnetic estimation (Wheeler coils, reluctance
networks, first-order motor constants) still dominates design practice,
and the incumbent free field solver (FEMM, 2004) is Windows-bound,
non-differentiable, and disconnected from modern CAD. We present a small
(~5 kLOC, zero-dependency-beyond-serde) 2D/axisymmetric field solver
built directly into an open-source CAD kernel, organized around three
disciplines: (1) **one symmetric finite-volume core** — every
formulation (axisymmetric and planar magnetostatics, electrostatics,
time-harmonic eddy currents) is `∇·(c∇u) = −s` with shared face
conductances, which makes the discrete adjoint reuse the forward solver
verbatim; (2) **every extracted quantity computed by two independent
routes** (energy vs flux linkage, induced charge vs energy, Maxwell
stress vs J×B), with the cross-route gap carried as machine-readable
provenance on a fail-closed design receipt; (3) **validation as a
ladder of exact anchors, published closed forms, and an independent
implementation**, with failure modes converted into regression tests.
Applied to a fabricated 70 mm PCB axial-flux motor, the solver confirms
the design pipeline's reluctance-network air-gap flux to 1.5%, predicts
a torque constant 15–20% below the shipped first-order estimate, and
attributes the gap quantitatively to the spiral tooth coils'
under-spanning of the pole (effective winding factor 0.598 vs the 0.866
slot factor the formula assumed) — a design lever invisible to the
formula.

## 1. Method

**Core.** Uniform node grids; per-face conductances `G_f` shared by the
face's two nodes (symmetric operator by construction); SOR with a
Chebyshev relaxation estimate and *scale-invariant* stopping (a
1e−30-scale source converges in the same sweep count as a unit one —
regression-tested); NaN fails closed in the convergence test. Materials
live on **cells**, sampled at cell centers (a sample point can never sit
on a region boundary — a float tie there once silently broke mirror
symmetry by 2% in an action–reaction test); each face conductance is the
parallel sum of its two flanking half-cells.

**Formulations.** Axisymmetric magnetostatics on the flux function
ψ = r·A_θ (∇·((ν/r)∇ψ) = −J_θ — FEMM's equation); planar magnetostatics
on A_z with permanent magnets as bound-current edge sheets
K = B_r/(μ₀μ_rec) and optional periodic-x for unrolled machines;
electrostatics on φ in both geometries; time-harmonic eddy currents as a
per-node imaginary diagonal −jωσ on the same stencil.

**Sampling.** Fields are exact derivatives of the bilinear interpolant:
divergence-free B pointwise (including through the axis cell, where the
physical ψ ∝ r² profile replaces the patch), conservative E. Pointwise
accuracy is staggered — second-order at cell centers, with a known
h/(2r) offset on nodes — and integral quantities average it out.

**Nonlinearity.** Single-valued arctangent B–H law (initial slope
μ₀μ_ri, deep-saturation slope μ₀). Picard iteration damped **on the
solved H** per cell: ν-damping has fixed-point derivative ~μ_r (measured
64×/iteration divergence) and B-damping explodes on the low-B branch of
MMF-driven cores (measured slope H/H_curve ≈ 50); the H update is exact
in one step whenever Ampère's law pins H.

**Time-harmonic.** Complex Gauss–Seidel: at the Chebyshev-optimal ω the
real-SOR iteration matrix is defective, and an imaginary diagonal of
just 1.4% of ΣG perturbs its coalesced eigenvalues past unit modulus
(measured divergence); ω = 1 is guaranteed by diagonal dominance.

**Adjoint.** For J(u) linear in u, one adjoint solve A·λ = ∂J/∂u prices
every parameter: dJ/dI_k = λᵀU_k (+ explicit terms), and
dJ/dG_f = −Δu_f·Δλ_f rolled to per-cell ν through face←cell incidence
weights **recorded by the assembly itself** (a bit-exact reconstruction
test guards drift), then to per-region μ_r. Saturable-region material
gradients are refused fail-closed (the secant ν depends on the field
through the Picard fixed point); geometry stays finite-difference.

## 2. Validation ladder

| class | problem | reference | result |
|---|---|---|---|
| exact anchor | infinite solenoid (Neumann) | Ampère incl. winding term | O(h²), measured orders 2.00/2.00/1.99/1.95; 1.48e−2 → 6.01e−5 over h = 2 → 0.125 mm |
| exact anchor | + μ_r = 50 core | Ampère | < 1% |
| exact anchor | + saturable core, 3 decades of drive | N·B_curve(nI)·πR² closed form | < 1% linear→knee→deep saturation |
| exact anchor | sheet-pair line | μ₀(gap + 2t/3)/w | < 0.5% |
| exact anchor | series dielectric | ε₀w/(d₁/ε₁+d₂/ε₂) | < 1e−6 (interface on node row: discretely exact) |
| closed form | coax capacitance | 2πε₀/ln(b/a) | < 0.2%; charge vs energy routes < 1e−6 apart |
| closed form | finite solenoid | Wheeler 1928 (±1%) | 0.15% |
| closed form | coaxial-loop mutual | Maxwell/Smythe elliptic | 3.0% (filament vs 1 mm² section, h = 1 mm) |
| closed form | coaxial-coil force | I₁I₂·dM/dz | J×B 0.08%; stress surface vs J×B 2.6%; action–reaction < 0.1% |
| closed form | staircased spheres C | 4πε₀ab/(b−a) | O(h) staircase, 5.0% → 2.1% monotone |
| **published benchmark** | permeable cylindrical shell, transverse field | Jackson 3rd ed., prob. 5.14: 4μb²/((μ+1)²b²−(μ−1)²a²) = 0.10182 | 32.7% → 5.1% → 1.7% over h = 1 → 0.25 mm (staircase ~O(h)); at h = 2 mm the 2 mm shell is one cell — the thin-feature floor, demonstrated |
| published closed form | AC rod at R/δ = 2 | 2J₁(kR)/(kR·J₀(kR)) (Stoll 1974) | < 2% amplitude and phase |
| analytic | slab skin effect | e⁻¹ and 1 rad per δ | < 3%; eddy-loss field vs circuit routes 2 ppm; L_ac→L_dc, R ∝ ω² |
| **independent code** | loop B field, 6 probes | `vcad_kernel_particle::field::b_ring` (elliptic integrals, separate implementation) | 0.8–3.9% |
| adjoint | dΛ/dI, dF/dI, dT/dI, dT/dBr, dJ/dμ | frozen-grid central differences; inductance-matrix identity | linear-in-I exact to solver tol; dΛ_j/dI_k ≡ L_jk to 1e−6; μ gradients ≤ 2e−3 |

Failure modes converted to regression tests: ψ = 0 truncation (error
must shrink with domain at fixed h), sheet-pair boundary gauge,
Wheeler-is-a-current-sheet, cell-based material sampling after the
float-tie symmetry break, probe conditioning off the dipole zero cone,
near-zero-QoI gradient comparisons, NaN-poisoned convergence.

## 3. Case study: the fabricated 70 mm PCB axial-flux motor

The as-built stator of `examples/pcb-motor` (9 slots / 6 poles, 10-turn
spiral tooth coils with radial conductors spread 2.6–7.2 mm off the
tooth axis, Y30 ferrite Ø15×3 discs, 1.0 mm gap, 2.7 mm steel irons),
unrolled at the 22.5 mm pitch radius on a periodic 560×81 grid:

- **Air-gap flux under a pole center: 0.201 T solved** vs the design
  pipeline's reluctance-network 0.204 T raw — the MEC confirmed by a
  field solution to 1.5%.
- Torque exactly linear in current; Maxwell-stress line vs J×B routes
  agree to 1.6% at every drive.
- **Kt = 3.13 mN·m/A** vs the shipped first-order estimate 3.70 (at
  fringing-derated flux) and 5.31 (at solved flux).
- The gap is *explained*, not just observed: the as-built spiral coil's
  current-weighted span is ~75°e, giving an effective winding factor
  `∫sin(πs/P)ds` = **0.598**, not the 0.866 full-tooth slot factor the
  formula assumes. Substituting it brackets the field solve
  (2.83…3.67 mN·m/A, derated↔solved flux conventions). Design lever:
  push spiral copper outward (fatter outer turns, hollow center).

Stated omissions of the slice (each a caveat on the emitted receipt
claim): curvature of the unrolled annulus, radial end fringing, linear
steel, statics, area-equivalent rectangular magnets.

## 4. Receipts

Every claim (`inductance_h`, `capacitance_f`, `force_n`, `torque_nm`,
`stored_energy_j`) is emitted under `vcad.em-claims/1` with formulation,
grid, SOR tolerance and sweeps, Picard iterations when nonlinear, and
the **cross-route residual** — the two-independent-routes gap — as
provenance. `compare()` binds bench measurements fail-closed
(Holds/Violated/Unmeasured; an unmeasured receipt never passes; a
measurement matching no claim is an error). The measurement pack
(`docs/em-measurement-pack.md`) binds the motor's Kt via back-EMF and
the air-gap flux via a Hall probe.

## 5. Limitations and roadmap

2D/axisymmetric only (3D is a different animal — out of scope);
staircased curved boundaries (first-order — every curved claim carries a
convergence bracket); no hysteresis; phasor and B–H do not compose
(harmonic balance unimplemented); source windings carry no internal skin
effect; the nonlinear fixed-point adjoint is future work (saturable
material gradients refuse, fail-closed). Next: kernel BRep extraction to
the spec seam, `vcad-receipt`/MCP registration, shape derivatives on
frozen masks, multigrid for high-contrast μ.

## Reproduction

```
cargo test -p vcad-kernel-em                                  # the ladder (54 tests)
cargo run --release -p vcad-kernel-em --example convergence   # §2 tables
cargo run --release -p vcad-kernel-em --example ladder_numbers
cargo run --release -p vcad-kernel-em --example motor_torque  # §3
```
