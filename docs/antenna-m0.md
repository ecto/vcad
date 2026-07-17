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
- **M3 — the `.vcad` seam. DONE** (`spec::AntennaSpec`, `ecad`). Serde
  schema (`#[serde(tag = "type")]`, matching IR conventions) in which
  every numeric field is a literal **or a named document parameter**;
  fail-closed resolution (unbound name = error, never a default);
  `parameter_names()` for enumeration. All named parameters are geometric
  and price their gradients through `adjoint::z_in_gradient`. The ecad
  seam: `strip_equivalent_radius_mm` (flat strip of width w ↔ round wire
  of radius w/4, the conformal-mapping equivalence) +
  `add_trace_as_wire`; board-side extraction stays on the vcad side,
  emitting this schema. A 78 mm × 1.6 mm trace monopole resolves and
  resonates at ℓ/λ = 0.24 in the free-space model — with the FR-4 shift
  called out as the M1.5 gap, not hidden.
- **M4 — receipt claims. DONE** (`receipt::predicted_claims`,
  `vcad.antenna-claims/1`): `s11_db_at_band`, `z_in_re/im`,
  `resonance_in_band` (+ `resonant_frequency` only when it is 1),
  `bandwidth_10db` (0 with an explicit note when the dip never reaches
  −10 dB — stated, never omitted), `gain_dbi`, `radiation_efficiency` —
  every claim carrying `basis: "predicted"`, its caveats (the substrate
  caveat verbatim on every number FR-4 would move), and provenance with
  the **kernel validity margins** (min Δ/4a, max Δ/(λ/8), max ka at band
  top, quadrature orders, grid). Fail-closed: claims are never emitted
  for a mesh outside kernel validity, and an off-resonance band says so
  in-claim instead of defaulting.
- **M5 — the NEC-2 face-off. DONE** (`tests/nec2_benchmarks.rs`,
  `docs/antenna-paper-draft.md`). References transcribed verbatim from
  the NEC-2 Manual Part III sample line-printer outputs (Burke & Poggio,
  LLNL; WDBN v0.92 pp. 83–93). Example 1 (0.5λ dipole, a = 0.001λ): NEC
  82.70 + j46.31 vs ours 86.18 + j46.27 — **|ΔZ|/|Z| = 3.7%, reactance
  within 0.04 Ω**. Example 2 (a = 1e−5 m, current-slope source, swept
  200/250/300 MHz): every point within **4% of |Z|** across a reactance
  swing from −632 Ω through resonance (26.58−j632.1 → 25.61−j608.1;
  47.14−j272.4 → 45.57−j261.8; 80.55+j45.71 → 78.17+j45.42). Example 3
  (vertical λ/2 over ground with **a = 0.3 m** — Δ/a = 1.85, ka = 0.19,
  the case NEC-2 itself needed its extended-kernel EK card for): our
  standard-kernel gates **refuse it fail-closed by name** — the correct
  answer until the extended kernel lands (queued). Paper-draft skeleton
  with every number reproducible from `cargo test`.
- **M6 — measurement pack. DONE** (`nanovna`, `receipt::compare`,
  `docs/antenna-measurement-pack.md`). A 915 MHz PCB monopole (78 ×
  1.6 mm trace over a pour, emitted-through-ecad board spec'd in the
  pack doc) whose free-space claims land **on band by geometry**:
  resonance 913.1 MHz, S11 −16.2 dB, Z = 37.5 + j5.5 Ω, 107 MHz
  bandwidth, 5.16 dBi. Touchstone `.s1p` parsing (RI/MA/DB, Hz–GHz,
  fail-closed by line number), `measurements_from_s1p` reducing a sweep
  to claim-named measurements through the implied impedance (any
  reference renormalized), and `compare()` with Holds / Violated /
  Unmeasured verdicts: stray measurement names are errors, one-port
  unmeasurables (gain, efficiency) read Unmeasured every time, and
  `fully_verified` requires all-measured-all-holding. The whole loop is
  rehearsed in `tests/measurement_pack.rs` against model-generated
  sweeps — including the **0.72× substrate-downshift rehearsal** where
  the frequency claims read Violated, which is exactly what the real
  FR-4 board is expected to do: that violation is the M1.5 ε_eff
  measurement, named in advance. Hardware BOM ≈ $100 (NanoVNA + boards).

## Non-goals

This crate does not claim patch-antenna, aperture, or full-wave 3-D EM
(FDTD/FEM) capability. It covers the wire-antenna family the thin-wire
EFIE covers — which happens to include most of what hams, IoT boards, and
RFID tags actually fly — and it says its validity limits out loud.
