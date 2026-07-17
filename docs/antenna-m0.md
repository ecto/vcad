# Antenna M0: thin-wire method-of-moments — impedance, S11, gain from wire geometry

`vcad-kernel-antenna` makes vcad a design tool for the wire-antenna family:
dipoles, monopoles, loops, yagis, inverted-F and meander PCB antennas. All
of them reduce to the same loop —

> wire geometry → currents → input impedance / S11 / far-field gain

— and the incumbent tool for that loop is **NEC-2: 1981 Fortran**, punched
card-style input decks, no gradients, no connection to CAD geometry or a
receipt system. The same ancient-incumbent shape `vcad-kernel-particle`
just replaced for charged-particle optics (SIMION lineage), one crate over.
The antenna version has the cheapest hardware-validation loop in the whole
portfolio: a PCB antenna costs a few dollars through the repo's existing
gerber pipeline, and a ~$100 NanoVNA measures S11 to fractions of a dB.

## M0 scope (and honesty)

**In scope:** perfectly conducting thin wires in free space.

- Geometry: straight wires, open polylines, closed loops (degree-2 chains),
  authored in millimeters; endpoints deduplicated so wires connect by
  construction. Multi-wire junctions (degree ≥ 3) **fail closed** at M0.
- Formulation: mixed-potential thin-wire EFIE, **triangular bases +
  Galerkin testing** (justified against NEC-2's sinusoidal point-matching
  in the `mom` module docs: structural reciprocity, junction-ready bases,
  no numerically-differentiated kernel — at the price of a somewhat denser
  mesh, which is cheap at N ≤ a few hundred).
- Kernel: reduced thin-wire kernel `R̃ = sqrt(|r−r′|² + ā²)` with
  symmetrized radius `ā² = (a_m² + a_n²)/2`, so the Galerkin matrix is
  exactly symmetric even for mixed-radius meshes. Each unordered segment
  pair is integrated once and assembled into both orientations —
  reciprocity holds to machine precision *by construction*, not by
  averaging.
- Quadrature: Gauss–Legendre outer; inner integrals split
  `e^{−jkR̃}/R̃ = smooth + 1/R̃ − jk` with the `1/R̃` ramp integrals in
  closed form (`asinh`), so self/adjacent terms carry their near-log
  singularity analytically.
- Linear algebra: hand-rolled complex pairs + dense LU with partial
  pivoting. No external dependencies.
- Excitation: delta-gap voltage source at any interior node; `Z_in(f)`,
  S11 against any real reference, frequency sweeps, bisection resonance
  search.
- Far field: radiation integral over the solved piecewise-linear currents;
  gain, directivity, radiated power (spectral quadrature over the sphere),
  and radiation efficiency as an **energy-balance cross-check** (far-zone
  integral vs feed power — two independent paths that must agree).

**Fail-closed validity gates** (hard errors, never silent degradation):
segment length ≥ 4×radius (thin-wire kernel; NEC-2's manual asks Δ/a > 8
for its standard kernel — we gate at 4 and the convergence study names the
floor), segment length ≤ λ/8 (λ/20 recommended), k·a ≤ 0.1, junctions,
empty meshes, non-bracketed resonance searches, singular systems.

**Out of scope at M0** (each is a milestone below): junctions, ground
planes, **dielectrics** — a PCB-trace antenna prediction at M0 is
**first-order only**: FR-4 pulls resonance down by ≈ `1/√ε_eff` (roughly
30–40% for microstrip-like traces) and M0 does not pretend otherwise. The
ε_eff correction is flagged M1.5 and until it lands, PCB predictions are
trends, not numbers. Also out: ohmic loss (radiation efficiency ≡ 1 for
PEC), curved-segment geometry, surface-patch conductors.

## Validation ladder (all in `cargo test -p vcad-kernel-antenna`)

| rung | result | published reference |
|---|---|---|
| Half-wave dipole Z_in at ℓ = λ/2 | R, X in [70, 92] / [30, 58] Ω | Balanis §4.6: 73 + j42.5 ideal-sinusoidal; finite-radius MoM reads higher R at exactly λ/2 |
| Resonance | ℓ/λ = 0.4790, R = 71.9 Ω, X ≡ 0 | Balanis: 0.46–0.49 λ, ~65–73 Ω |
| Broadside directivity | 2.138 dBi | 2.15 dBi (D = 1.643) |
| Short dipole (ℓ = 0.04 λ) | R_r ≈ 20π²(ℓ/λ)², strongly capacitive | Balanis §4.3 (triangular current) |
| Small loop (16-gon, C = 0.08/0.12 λ) | R_r within 25% of 320π⁴(A/λ²)², ratio ≈ (C₂/C₁)⁴, D ≈ 1.76 dBi, inductive | Balanis §5.2: 20π²(C/λ)⁴ |
| Segment convergence | ≤ 2% by N = 32 vs N = 48; thin-wire floor **errors** at Δ < 4a | self-named floor |
| Reciprocity | Y₂₁ = Y₁₂ to ~1e−12 (two-dipole link + same-wire ports) | structural (Galerkin symmetry) |
| Energy balance | P_rad/P_in = 1.0000 at resonance; within 3% at ℓ/λ = 0.3 and 0.7 | far-zone vs feed-power cross-check |
| Pattern structure | θ-polarized, azimuth-uniform, axial null > 30 dB | dipole physics |

## Benchmark: the simulated NanoVNA

`cargo run --release -p vcad-kernel-antenna --example dipole_sweep`

A 2 × 0.5 m-arm dipole (1 mm wire radius, 40 segments, ℓ/a = 1000), swept
0.7–1.3 × resonance, CSV out:

- resonance **143.61 MHz** → ℓ/λ = **0.4790** (the textbook "a real dipole
  is ~4–5% shorter than λ/2" falls out of the integral equation)
- Z_in at resonance **71.9 + j0.0 Ω** → S11 vs 50 Ω = **−14.9 dB**,
  exactly the Γ = (72−50)/(72+50) the impedance implies
- broadside directivity **2.138 dBi**, radiation efficiency **1.0000**
- X swings −329 Ω (capacitive, short side) → +337 Ω across the sweep;
  R climbs 26 → 206 Ω; broadside gain stays within ~0.5 dB of the dipole
  value — impedance mismatch, not pattern, is what moves S11 here.

## Milestone ladder

- **M1 — junctions + ground plane. DONE.** Degree-`d` junctions carry
  `d − 1` KCL-spanning bases (reference-branch pairing — the signed-half
  machinery from M0, unchanged); perfect ground plane via image sources
  (mirrored geometry, flipped current, testing on the real conductor
  only), with grounded wire-end bases so a monopole feeds at its base.
  Validation (`tests/ground_and_junctions.rs`): quarter-wave monopole
  resonates at ℓ/λ = **0.2395** — exactly half the dipole's 0.4790 — with
  R = **35.95 Ω** (= Z_dip/2 to 2%), **5.15 dBi** at the horizon
  (published 5.16), zero field below the PEC horizon; horizontal dipole
  R(h) tracks R_self − R_mutual(2h) including the h = 0.1 λ collapse;
  **image theory asserted as an algebraic identity** (ground solve ≡
  antisymmetric-twin solve through a different code path, to 1e−6);
  3-element yagi beams **7.51 dBi forward, F/B 19.3 dB** off parasitic
  coupling alone; folded dipole steps up to **285 Ω** (published 4 × 73 ≈
  292); a T-hat junction drags monopole resonance 143.7 → 82.8 MHz;
  reciprocity and the 1.0000 energy balance survive junctions and images.
  Numerical lesson preserved in the fill: a segment's kernel to its own
  image depends on s + t (not |s − t|), so the Gauss-node reflection
  symmetry that protects direct self terms does not apply — the
  self-image block is explicitly symmetrized (the exact integrals are
  equal by mirror isometry; the discretization just dropped it).
- **M1.5 — substrate honesty for PCB antennas.** Effective-permittivity
  correction for traces over a dielectric (quasi-static ε_eff), stated as
  a correction with its own validity limits — this is what turns "PCB
  antenna trends" into "PCB antenna predictions".
- **M2 — gradients. DONE** (`adjoint::z_in_gradient`). The symmetric
  matrix makes the input-impedance adjoint *free*: `Zᵀλ = e_f` with
  `Z = Zᵀ` gives `λ = I/V₀`, so the gradient is the variational identity
  `dZ_in/dp = Iᵀ(∂Z/∂p)I / I_f²` — one solve total, any number of
  parameters, each priced at two matrix fills (central FD **on the fill
  only**, never through the LU). Parameters are per-node velocity fields;
  `perturbed_mesh` moves coordinates under a **frozen topology**, so the
  hidden-parameter lesson from the particle crate is structural here, not
  procedural. Fails closed if a parameter would move a grounded node off
  the plane. Validated: adjoint = frozen-segmentation FD through the full
  solve to < 1e−4 (free space, and through images + junctions on the
  hatted monopole); rigid-translation gauge (zero gradient); sign physics
  (stretch → inductive; hat growth → dX/dp > 0, i.e. f_res down, why
  short verticals wear hats). Then the gradient designs the antenna:
  **Newton on arm strain retunes a 10%-detuned dipole to X = 0 in 3–4
  steps**, landing within 0.01% of the bisection resonance.
- **M3 — the `.vcad` seam.** Serde `AntennaSpec` with named parameters,
  fail-closed resolution (unbound name = error), plus the PCB-trace →
  wire-grid adapter (equivalent-radius rule a ≈ 0.335·w for flat traces)
  documented as the ecad seam.
- **M4 — receipt claims.** `vcad.antenna-claims/1`: `s11_db_at_band`,
  `z_in_ohm`, `gain_dbi`, `bandwidth_mhz` with full provenance (segment
  count, kernel validity margins, frequency grid) and spelled-out caveats
  (no substrate at M0/M1, etc.), in the mold of
  `vcad.particle-claims/1`.
- **M5 — the NEC-2 face-off.** Reproduce published NEC-2 validation cases
  (the manual's dipole tables and a folded dipole / yagi case), table the
  deltas, and write the paper-draft skeleton.
- **M6 — measurement pack.** Design a 915 MHz (or 2.45 GHz) PCB
  monopole/IFA as a board the repo's ecad pipeline can emit, plus
  `compare()` binding NanoVNA S11 sweeps to predicted claims with
  Holds / Violated / Unmeasured verdicts — fail-closed, Violated is a
  publishable result about the model, and the whole loop costs about as
  much as a nice lunch.

## Non-goals

This crate does not claim patch-antenna, aperture, or full-wave 3-D EM
(FDTD/FEM) capability. It covers the wire-antenna family the thin-wire
EFIE covers — which happens to include most of what hams, IoT boards, and
RFID tags actually fly — and it says its validity limits out loud.
