# An open, receipt-native tolerance stackup engine (draft)

**Working title:** *The missing open GD&T engine: deterministic tolerance
stackup analysis with exact sensitivities, honest bonus tolerance, and
fail-closed yield claims*

**Status:** draft skeleton with all current numbers; prose to be
tightened. Everything quantitative below reproduces with fixed seeds from
`vcad-kernel-tolerance` (tests + `bearing_stack` / `comparison`
examples).

## Abstract

Every assembly drawing implies a probability: the chance that parts made
to its tolerances actually fit and function. Industry answers this with
spreadsheets (two columns of ±'s and a gut call) or with closed
commercial bolt-ons (CETOL, 3DCS, VisVSA) whose math cannot be audited.
There is no good open tool in this space. We present an open stackup
engine that computes worst-case, RSS, and seeded Monte Carlo analyses
over assembly chains; reports **exact closed-form sensitivities** (the
gap, σ, and yield derivatives every allocation decision needs — linear
chains make adjoint machinery unnecessary, a fact we exploit rather than
apologize for); models position tolerance at MMC with the bonus handled
honestly (bonus changes conformance, not physics); allocates tolerances
by minimizing cited cost models under a yield floor via an exact KKT
bisection; and emits every result as a fail-closed claim set
(`vcad.tolerance-claims/1`) carrying its provenance: sample count, seed,
tolerance-to-σ convention, and per-contributor distribution sources.
Probabilities without error bars are unrepresentable in the API.

## 1. The problem and the incumbents

- The stackup spreadsheet: WC and RSS columns, no yield, no
  sensitivities, no provenance; the ±tol ↔ σ link is implicit and
  usually wrong.
- Commercial tolerance CAE: capable but closed, expensive, and
  disconnected from open CAD kernels; results are unauditable numbers.
- The open-source landscape: effectively empty (a survey belongs here —
  flagged).
- Our claim: the deliverable of tolerance analysis is a *receipt* — a
  yield claim with error bars, assumptions, and the ranked list of which
  dimension is killing it — and it should live inside the kernel, next
  to the geometry, under version control.

## 2. Model

Linear chains G = Σ aᵢxᵢ (§honesty for what linearity excludes);
contributors carry nominal, signed coefficient, drawing limits, a
deviation distribution (Normal / Uniform / TwoPoint), and a
**provenance tag** (assumed-under-convention vs measured). The
tolerance-to-σ convention (default ±tol = 3σ) is data, not folklore:
it appears on every receipt. ISO 2768-1 f/m/c general tolerances are
bundled and cited for defaults. 2D/3D vector loops project onto a
measure direction (exact for translations; small-angle for rotations
with the r·θ²/2 bound stated); the Rayleigh structure of radial fits is
handled exactly in the GD&T module, never silently linearized —
projection provably overstates radial fit probability (~1.9 points at
c/σ = 2.7 in our fixture).

## 3. Analyses

- **Worst-case:** interval arithmetic over drawing limits.
- **RSS:** exact moment propagation for any distribution mix; the
  Φ-based yield is exact for all-normal chains and CLT otherwise — the
  result carries an `all_normal` flag.
- **Monte Carlo:** hand-rolled xoshiro256++ (Blackman & Vigna 2021)
  seeded via SplitMix64; Marsaglia-polar normals; Agresti–Coull (1998)
  standard errors on every probability plus a batch-means cross-check.
  Same seed, same bits, any platform.
- **Capability:** Cp/Cpk and yield on the A&S 7.1.26 erf (max abs error
  1.5×10⁻⁷ — the approximation bound rides on the claim as its stated
  uncertainty).
- **Exact sensitivities:** ∂G/∂nomᵢ = aᵢ; ∂σ_G/∂σᵢ = aᵢ²σᵢ/σ_G;
  variance shares; ∂Y/∂nomᵢ and ∂Y/∂σᵢ by the Φ chain rule. Validated
  against finite differences (validating the algebra, not approximating
  it).

## 4. GD&T at MMC, honestly

Bonus tolerance changes **conformance, not physics**: the position
scatter of a process does not improve because the hole came out big —
but the big hole really does clear a worse misalignment, and inspection
against the dynamic gauge truncates the shipped population. Keeping
those three statements separate lets us reproduce, by simulation, the
Y14.5 theorem that gauged parts with compatible virtual conditions fit
**every single time** (50,000/50,000 in the test), while the ungauged
population genuinely fails at the predicted rate. Fixed-fastener
virtual-condition worst case sits beside the statistical fit rate on
the same receipt: in our bolt-circle fixture the VC check fails by
0.05 mm while the true assembly fit rate is 87.0% — the entire reason
MMC and statistical tolerancing exist.

## 5. Allocation

Minimize Σ Cᵢ(tᵢ) subject to yield ≥ Y over cited cost families
(reciprocal, reciprocal-squared — Spotts 1973; exponential — Speckhart
1972; survey lineage Chase & Greenwood 1988). Yield is monotone in σ_G,
so the constraint is σ_G ≤ σ_max (bisection on the exact Φ); KKT
stationarity has closed forms per contributor for the reciprocal
families; one outer λ-bisection with box clamping solves the whole
problem deterministically. Verified against the closed-form Lagrange
solution (λ and every tᵢ to 1e-6) and the proportional-scaling
baseline.

## 6. Validation ladder (all in `cargo test`)

- PRNG determinism, decorrelation, moments, tail masses.
- erf/Φ vs reference values at the stated 1.5e-7 bound.
- RSS ≡ MC within error bars (all-normal); stated CLT band (mixed).
- WC contains every MC sample (fixed seeds; bounded dists by theorem,
  normal chains at ~6.7σ_G).
- Hand-computed textbook chain at 1e-12.
- 1/√N error scaling across 24 disjoint seeds.
- **Irwin–Hall exact benchmarks**: a 3-uniform chain against the exact
  CDF — MC lands within error bars; the RSS/CLT error is measured
  (7×10⁻³ at a ±2.5σ requirement) and its **sign is proven**: Φ
  under-reads bounded chains at the tails (RSS is conservative there),
  and at the worst-case bounds the exact yield is 1 while the normal
  model still leaks. A triangular (n = 2) case is hand-computed
  (0.875 exactly).
- **The √n law** — WC half-width over RSS 3σ half-width equals √n —
  asserted to 1e-12 for n = 4, 9, 16, 25. This ratio is the economic
  argument for the whole field, so it is a test, not a slogan.
- Rayleigh closed forms for radial fits (bolt circle, conformance,
  MMC bonus quadrature).

## 7. Results

**Comparison study** (n equal ±0.1 contributors, requirement 1.0 ±
0.31): worst-case declares the design unbuildable from n = 4 on; the
true yield at n = 10 is still 99.67 ± 0.02%. The WC/RSS ratio column
reads √n to machine precision.

**Bearing stack** (housing + 2 vendor bearings + shoulder + two-supplier
spacer + circlip, ISO 2768-m defaults, mixed distributions): WC fails
both ends (−0.26 mm jam margin, +1.30 vs 0.75 rattle limit); RSS/MC
agree on 95.33%/95.39 ± 0.05% yield; the housing bore owns 54.3% of the
variance; the exact yield gradient's *free* re-centering move (spacer
+0.122 mm) lifts yield to 99.01% without tightening anything; the
allocator then buys 99.73% at minimum cost (housing 0.300 → 0.237,
shoulder 0.200 → 0.175), beating proportional scaling by 0.2% at equal
cost coefficients and by 6.6% under 30× cost asymmetry — allocation's
edge grows exactly where hand methods are blindest.

**Receipts:** every result above emits as `vcad.tolerance-claims/1`
with n, seed, convention, and per-contributor sources; `compare()`
binds measurements with Holds/Violated/Unmeasured verdicts and an
unmeasured receipt never passes.

## 8. Honesty / limitations

Linear chains (stated small-angle bound for loops); independence
assumed (fixture-correlated dims are future work); distributions are
assumptions until measured (the measurement pack flips sources to
`measured`); the ±tol ↔ σ convention is the biggest lie in most stackups
and is therefore receipt provenance; no datum reference frames, no
composite frames, no profile simulation (the full-3D-gauge feature set
is out of scope and named as such).

## 9. External follow-ups (flagged, not faked)

- Cross-validation against published worked examples (the
  Fortini/Chase–Greenwood one-way clutch) with the source tables in
  hand — not reconstructed from memory.
- Cross-validation against a commercial tool run (CETOL/3DCS licenses
  required).
- The `vcad-receipt` schema registration + MCP tools
  (`analyze_tolerance`, `allocate_tolerance`) — cross-crate codegen PR.
- Correlated contributors; truncated-normal (post-inspection)
  distributions as first-class citizens.

## 10. Availability

`crates/vcad-kernel-tolerance` in the vcad repository. Zero
dependencies beyond serde; every number in this draft regenerates from
fixed seeds via `cargo test -p vcad-kernel-tolerance` and the two
examples.
