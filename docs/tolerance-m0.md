# Tolerance stackup M0: worst-case, RSS, and Monte Carlo over assembly chains

`vcad-kernel-tolerance` makes vcad answer the question every assembly
drawing begs and almost no tool answers honestly: **does this fit, and at
what yield?** The incumbent workflow is a spreadsheet and prayer — a
column of ±'s summed two ways and a gut call. The commercial tools that do
better (CETOL, 3DCS, VisVSA) are expensive bolt-ons to CAD systems that
don't share their math; there is no good open tool in this space at all.
This crate is the M0 of a deterministic, receipt-native stackup engine
that lives inside the kernel next to the geometry it prices, and its
deliverable claim is the portfolio's purest product sentence: *"this
assembly fits with 99.7% yield."*

## M0 scope (and honesty)

**In scope:** linear dimension chains, analyzed three ways.

- **Model** (`stackup`): contributors with a nominal, a signed
  coefficient, drawing limits, and a deviation distribution — Normal
  (machined dims), Uniform (vendor lots), or TwoPoint /
  "Bernoulli-shifted" (two suppliers, two mold cavities, seated-or-not).
  Each contributor records its distribution's **provenance**
  ([`DistributionSource`]): assumption or measurement.
- **Worst-case**: interval arithmetic over drawing limits. Guaranteed
  bounds; width grows as Σtᵢ while real scatter grows as √Σtᵢ² — the
  gap between those two laws is this whole field.
- **RSS**: exact linear variance propagation (μ_G = Σaᵢμᵢ,
  σ_G² = Σaᵢ²σᵢ²) — the *moments* are exact under independence for any
  distribution mix; the Φ-based *yield* is exact when all contributors
  are normal and a CLT approximation otherwise (the result carries an
  `all_normal` flag rather than letting you forget).
- **Monte Carlo**: seeded deterministic sampling (hand-rolled
  xoshiro256++ seeded via SplitMix64 — Blackman & Vigna 2021, Steele,
  Lea & Flood 2014; no `rand` dependency, reproducibility must not
  hinge on someone else's version bump). Fit probabilities carry an
  Agresti–Coull standard error (Agresti & Coull 1998 — never reports
  SE = 0 at k = 0 or n, because "no failures observed" is not
  certainty), plus a batch-means SE cross-check. **Probabilities
  without error bars are unrepresentable in the API** — the
  `ProbabilityEstimate` type has no error-bar-free constructor.
- **Capability** (`capability`): predicted gap distribution (μ, σ),
  Cp/Cpk against the requirement, Φ-based yield on a hand-rolled erf
  (Abramowitz & Stegun 7.1.26, max |error| 1.5×10⁻⁷ — cited, and the
  bound is asserted against reference values in the tests).
- **Exact sensitivities** (`sensitivity`): ∂G/∂nominalᵢ = aᵢ,
  ∂σ_G/∂σᵢ = aᵢ²σᵢ/σ_G, per-contributor variance shares, and exact
  ∂yield/∂nominalᵢ, ∂yield/∂σᵢ via the Φ chain rule. **These are closed
  forms, not adjoints** — linearity hands us every derivative exactly,
  and the FD cross-check in the tests validates the algebra, not an
  approximation. Ranked output = "which dimension is killing the
  yield."
- **Vector loops** (`loops`): 2D/3D legs projected onto a measure
  direction (aᵢ = v̂ᵢ·d̂) — exact for translational legs, first-order
  small-angle for rotations (dropped term ~ r·θ²/2; at |θ| ≤ 2° the
  error is ≤ 1.7% of the kept term; past ~5°, model the mechanism).
- **ISO 2768-1 general tolerances** (`iso2768`): the f/m/c linear-dim
  table bundled and cited, fail-closed outside its domain.

**The honesty section** (each of these is stated in API docs, not just
here):

- **The ±tol ↔ σ convention buries more products than any solver bug.**
  The default ±tol = 3σ (`SigmaConvention::ThreeSigma`) assumes a
  Cp = 1.00 process that ships 0.27% of parts out of limits. A Cp = 1.33
  supplier is 4σ; Six Sigma is 6σ. Every yield number downstream
  inherits this choice, so it is provenance on the receipt (M4), never a
  silent default.
- **Independence is assumed.** Two dimensions cut in one fixture setup
  are correlated, and RSS and MC are both wrong about their sum.
  Correlation modeling is future work.
- **Distributions are assumptions until measured.** M6 binds the repo's
  3DP print-then-measure loop so coupon scatter replaces assumptions.
- **Worst-case containment of normal tails is conventional.** A normal
  contributor genuinely exceeds its drawing limits 0.27% of the time
  (that's what the convention *means*); the WC interval still contains
  every MC sample in practice because a chain-level escape needs every
  contributor near the same-signed extreme simultaneously (~6–7 σ_G
  out; P ≈ 1e-11) — the ladder asserts it with fixed seeds.
- **Linear projection is optimistic for radial fits.** The magnitude of
  a 2-D position error is Rayleigh-distributed; projecting onto one
  direction overstates fit probability (at c/σ = 2.7, by ~1.9
  percentage points). `tests/bolt_circle.rs` quantifies it against the
  exact closed form; radial fits belong to the GD&T module (M1), not a
  silent linearization.

## Validation ladder (all in `cargo test -p vcad-kernel-tolerance`)

- xoshiro256++/SplitMix64: seed determinism (bit-exact streams),
  adjacent-seed decorrelation, zero-seed health, uniform and normal
  moments + tail masses (31.73/4.55/0.27% at 1/2/3σ).
- erf/Φ against reference values at the cited 1.5×10⁻⁷ bound; yield at
  textbook z (±1σ, ±1.96σ, ±2.576σ, ±3σ).
- Distribution moments vs closed forms; sampling convergence.
- **RSS vs MC self-consistency** (`tests/validation.rs`): 5-contributor
  all-normal chain agrees within 4 standard errors at n = 200k; a mixed
  normal/uniform/two-point chain holds a stated CLT band (5×10⁻³).
- **WC contains every MC sample** on both chains, fixed seeds.
- **Textbook chain hand-computed**: 3-contributor stack asserted to
  1e-12 on μ/σ/WC/Cp/Cpk, yield to the erf bound.
- **1/√N scaling**: p̂ spread across 24 disjoint seeds halves from
  n = 4k → 16k (band [1.4, 2.9]), and the reported SE matches the
  empirical spread within ×1.8.
- **Bolt-circle** (`tests/bolt_circle.rs`): exact-model MC lands on the
  Rayleigh-CDF integral within error bars; the linearized projection is
  proven optimistic across the clearance sweep; the virtual-condition
  worst case fails while the statistical fit rate is ~90% — the entire
  reason MMC/statistical analysis exists, reproduced from first
  principles.
- Fail-closed paths: empty chains, duplicate names, zero coefficients,
  negative tolerances, distributions wider than their drawing band,
  unbounded requirements, degenerate (all-σ = 0) chains, and too-few
  MC samples are all errors, never defaults.

## Benchmark: the bearing stack

`cargo run -p vcad-kernel-tolerance --example bearing_stack`

A gearbox input-shaft axial stack: housing bore depth (ISO 2768-m
normal), two unilateral vendor-band bearing widths (uniform), a machined
shoulder (2768-m normal), a two-supplier ground spacer (two-point mix
inside its band), and a circlip (normal). Requirement: axial play ∈
[0.05, 0.75] mm.

| analysis | result |
|---|---|
| Worst case | gap ∈ [−0.260, 1.300] mm — **fails both ends** (jam and rattle both "possible") |
| RSS | μ = 0.5220 mm, σ = 0.1357 mm, Cp = 0.86, Cpk = 0.56, yield **95.33%** |
| Monte Carlo (n = 200k) | fit = 95.39% ± 0.05%, μ = 0.5217 ± 0.0003, σ = 0.1356 ± 0.0002 — brackets RSS |

Sensitivity ranking: housing bore depth owns **54.3%** of the gap
variance (σ = 0.1), the shaft shoulder 24.1%; everything else is single
digits. The exact yield gradient (∂Y/∂nominal = +0.71/mm on every
gap-consuming member) points at the real defect — the unilateral bearing
bands push the mean 0.122 mm above the requirement center — and the
worked re-centering move (spacer 12.000 → 12.122 mm) lifts yield from
95.33% to 99.01% **without tightening a single tolerance**. Tightening
the *right* σ's at minimum cost is M2.

The story in one sentence: worst-case says redesign it, statistics says
ship it and here is the number, and the sensitivity table says what to
fix first if the number isn't good enough.

## Milestone ladder

- **M1 — GD&T semantics that move fits.** Position tolerance at MMC
  with bonus tolerance modeled honestly (size distribution → effective
  zone), virtual-condition worst case, flatness/perpendicularity as
  contributor generators. Scoped subset, stated.
- **M2 — tolerance allocation.** Minimize manufacturing cost subject to
  yield ≥ target over per-contributor cost-vs-tolerance curves
  (reciprocal/exponential families, Chase–Greenwood lineage). The exact
  gradients make this a clean constrained optimization: Lagrange
  bisection with per-contributor closed forms + KKT box clamping.
  (`vcad-kernel-cost` models process-level cost — material + machine
  time — not cost-vs-tolerance, so the curves are bundled here and the
  seam documented.)
- **M3 — parameter seam.** Serde `StackupSpec` where every numeric is a
  literal or a **named document parameter**, fail-closed resolution
  (mirrors `vcad-kernel-particle::spec`); the documented adapter
  contract from vcad documents — dims from sketches, and existing
  `check_clearance` assertions become requirements. BRep extraction
  lands vcad-side, emitting this schema.
- **M4 — receipt claims.** `vcad.tolerance-claims/1`: fit_probability
  (with MC standard error), cpk, worst_case_margin_mm, sigma_gap — with
  provenance (n samples, seed, sigma convention, per-contributor
  distribution sources) and fail-closed `compare()`
  (Holds/Violated/Unmeasured; an unmeasured receipt never passes).
- **M5 — benchmarks + paper draft.** Exact-analytic fixtures (uniform
  convolutions have closed-form CDFs; √n WC/RSS width law), the
  WC-vs-RSS-vs-MC comparison study, `docs/tolerance-paper-draft.md`
  positioned as the missing open GD&T engine.
- **M6 — measurement pack.** Measured dimensional scatter (the repo's
  `predict_print`/`record_measurement` loop) fitted into contributor
  distributions (`DistributionSource::Measured`), predicted-vs-measured
  yield closed through `compare()`.

## Non-goals

This crate does not do full 3-D tolerance zone simulation (surface-level
GD&T with datum reference frames and simulated gauges — the 3DCS/CETOL
feature set) at M0, and says so. It prices the chains engineers actually
write down, exactly, with error bars, and it refuses to produce a number
without its assumptions attached.
