# A receipt-native, differentiable thin-wire method-of-moments solver inside a CAD kernel

*Draft skeleton — all numbers below are produced by
`cargo test -p vcad-kernel-antenna` and the crate's examples on the
commit that ships this file. Nothing here is aspirational.*

## Abstract

We present `vcad-kernel-antenna`, a thin-wire method-of-moments (MoM)
electromagnetic solver embedded in the vcad CAD kernel. It predicts input
impedance, S11, and far-field gain directly from wire geometry, replaces
the 1981-Fortran incumbent (NEC-2) for the wire-antenna family inside
this toolchain, and adds three things the incumbent lineage never had:
**structural reciprocity** (a Galerkin matrix symmetric to machine
precision by construction), **adjoint gradients of input impedance with
respect to geometry** (free, by the same symmetry), and **fail-closed
validity gates plus receipt claims** whose provenance names the kernel's
own limits. Validation spans closed-form antenna theory (Balanis),
NEC-2's published sample outputs (agreement within 4% of |Z| across a
−632 Ω → +46 Ω reactance swing), and a planned $100 hardware loop (PCB
antenna + NanoVNA).

## 1. The loop, and the incumbent

wire geometry → currents → Z_in / S11 / gain. The incumbent is NEC-2:
1981 Fortran, card-image input, no gradients, no validity refusal, no
connection to the geometry system that produced the wires. Same
ancient-incumbent shape as SIMION for charged-particle optics, which the
sibling crate `vcad-kernel-particle` addressed; this crate is the same
milestone ladder applied to electromagnetics, with the cheapest hardware
validation in the portfolio.

## 2. Method

- **Formulation:** mixed-potential thin-wire EFIE; triangular bases +
  Galerkin testing. Both derivatives integrate by parts onto the bases —
  no numerically differentiated kernel. Junction nodes of degree d carry
  d−1 reference-branch bases (KCL-exact); wire ends on a ground plane
  carry one-half bases whose other half is the image.
- **Kernel:** reduced thin-wire kernel `R̃ = sqrt(|r−r′|² + ā²)`,
  `ā² = (a_m² + a_n²)/2` — the radius symmetrization keeps Z exactly
  symmetric for mixed-radius meshes.
- **Symmetry as an invariant, not an accident:** each unordered segment
  pair is integrated once and assembled into both orientations;
  self-image blocks (whose kernel depends on s + t, defeating the
  Gauss-node reflection symmetry that protects direct self terms) are
  explicitly symmetrized under the mirror isometry that makes their
  exact values equal. Measured asymmetry: < 1e−12 of matrix scale, with
  junctions and ground images in play.
- **Quadrature:** outer Gauss–Legendre; inner integrals split
  `e^{−jkR̃}/R̃ = smooth + 1/R̃ − jk` with closed-form (`asinh`) ramp
  integrals for the near-log singular part.
- **Ground plane:** image sources (mirrored geometry, flipped current),
  testing restricted to the real conductor; hemisphere power integration.
- **Linear algebra:** hand-rolled complex pairs, dense LU with partial
  pivoting; the factorization is shared by all right-hand sides.
- **Gradients:** with `Z I = V₀ e_f` and Z symmetric, the adjoint of
  `Z_in = V₀/I_f` is `λ = I/V₀`, so
  `dZ_in/dp = Iᵀ(∂Z/∂p)I / I_f²` — no extra solve. `∂Z/∂p` by central
  differences on the fill under a frozen topology (`perturbed_mesh`).
- **Fail-closed gates:** Δ ≥ 4a (thin-wire kernel), Δ ≤ λ/8 (sampling),
  k·a ≤ 0.1, junction/ground contact rules, resonance brackets,
  singularity thresholds. Gates error; they never degrade.

## 3. Validation ladder (all asserted in CI)

| rung | this solver | reference |
|---|---|---|
| dipole resonance (ℓ/a = 1000) | ℓ/λ = 0.4790, R = 71.9 Ω | Balanis: 0.46–0.49 λ, 65–73 Ω |
| half-wave directivity | 2.138 dBi | 2.15 dBi |
| short dipole R_r | 20π²(ℓ/λ)² ± few % | Balanis §4.3 |
| small-loop R_r (16-gon) | 320π⁴(A/λ²)², (C/λ)⁴ scaling | Balanis §5.2 |
| quarter-wave monopole | ℓ/λ = 0.2395, R = 35.95 Ω, 5.15 dBi | 0.5 × dipole; 5.16 dBi |
| image theory | ≡ antisymmetric twin solve, rel < 1e−6 | algebraic identity |
| horizontal dipole over PEC | R(h) = R_self − R_mut(2h); R(0.1λ) ≈ 20 Ω | Balanis Fig. 4.31 |
| folded dipole | 285 Ω resonant (4.0× step-up) | 4 × 73 ≈ 292 Ω |
| 3-element yagi | 7.51 dBi fwd, F/B 19.3 dB | NBS/amateur 7–9 dBi |
| top-hat junction | f_res 143.7 → 82.8 MHz | capacitive loading |
| reciprocity | Y₂₁ = Y₁₂ to ~1e−12 | structural |
| energy balance | P_rad/P_in = 1.0000 (dipole), ±3% off-resonance | far zone vs feed |
| segment convergence | ≤ 2% by N = 32; floor at Δ = 4a **errors** | self-named |
| adjoint gradient | = frozen-FD to < 1e−4; Newton retune in ≤ 5 steps | §2 identity |

## 4. The NEC-2 face-off

Reference values transcribed from the NEC-2 Manual Part III sample
outputs (Burke & Poggio, LLNL; WDBN v0.92, pp. 83–93). Equal electrical
geometry; NEC-2 runs its sinusoidal basis at the segment counts printed
in the manual, ours is triangular Galerkin at N = 64; sources differ
(delta gap vs current-slope discontinuity) — few-percent deltas are the
expected cost of those choices.

| case | NEC-2 printed | this solver | Δ\|Z\|/\|Z\| |
|---|---|---|---|
| Ex. 1 — 0.5λ dipole, a = 0.001λ, 7 seg | 82.6979 + j46.3060 | 86.18 + j46.27 | 3.7% |
| Ex. 2 — a = 1e−5 m, 200 MHz | 26.5762 − j632.060 | 25.61 − j608.1 | 3.8% |
| Ex. 2 — 250 MHz | 47.1431 − j272.372 | 45.57 − j261.8 | 3.9% |
| Ex. 2 — 300 MHz | 80.5511 + j45.7144 | 78.17 + j45.42 | 2.6% |
| Ex. 3 — vertical λ/2 over ground, **a = 0.3 m** (Δ/a = 1.85, ka = 0.19) | 106.44 + j99.06 (extended kernel, EK card) | **refused, fail-closed** (thin-wire gate names Δ < 4a) | — |

Example 3 is the interesting row: NEC-2 itself required its extended
thin-wire kernel for that geometry. This crate implements the standard
kernel with hard gates, so the *correct* output is a refusal that names
the violated limit — not a confidently wrong number. The extended kernel
is queued; the printed NEC value becomes reproducible then.

## 5. Gradients that design

Newton iteration on arm strain, priced by the adjoint identity, retunes
a 10%-detuned dipole onto a 143.61 MHz target in ≤ 5 solves, landing
within 0.01% of the bisection-search resonance. The same machinery
prices any named parameter of the serde `AntennaSpec` (M3), so a
document parameter in a `.vcad` file can be driven onto a target band.

## 6. Honesty: what this model does not contain

Free-space PEC wires only. No dielectrics — a PCB antenna prediction
through the `ecad` adapter (strip width w ↔ wire radius w/4) is
**first-order**: FR-4 pulls resonance down by roughly `1/√ε_eff`, tens
of percent, and every receipt claim carries that caveat verbatim. No
ohmic loss (radiation efficiency ≡ 1; the energy-balance number is a
cross-check, not a claim about copper). No finite ground planes, no
extended kernel, no curved segments, no surface patches. Each limit is a
named milestone or a named refusal, and the claims module
(`vcad.antenna-claims/1`) states the margins on every claim set.

## 7. The $100 hardware loop (M6)

A 915 MHz PCB quarter-wave monopole through the repo's gerber pipeline,
measured with a NanoVNA (~$100): `receipt::compare` binds the measured
S11 sweep to the predicted claims with Holds / Violated / Unmeasured
verdicts, fail-closed (an unmeasured receipt never passes; a Violated
claim is a publishable result about the model — the expected first
violation is the resonance downshift from the missing substrate, which
*is the M1.5 measurement*).

## Reproduction

```
cargo test -p vcad-kernel-antenna
cargo run --release -p vcad-kernel-antenna --example dipole_sweep
cargo run --release -p vcad-kernel-antenna --example pattern_cut
```
