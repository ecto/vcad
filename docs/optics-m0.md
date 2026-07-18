# Sequential ray-tracing lens design M0: exact traces, paraxial cross-checks, the achromat rediscovered

`vcad-kernel-optics` makes vcad a design tool for imaging optics —
singlets, achromatic doublets, camera/telescope objectives, relay and
collimation lenses. All of them reduce to the same loop —

> surface prescription → exact ray trace → spot-size figure of merit

— and the incumbent tools for that loop (Zemax/OpticStudio, Code V) are
decades old, closed, five-figure-per-seat, and non-differentiable
end-to-end. This crate is the M0 of a differentiable, receipt-native
replacement living inside the kernel next to the geometry that will
mount the glass.

## M0 scope (and honesty)

**In scope:** sequential geometric ray tracing.

- Surfaces: spherical + conic (κ), plane, aperture stop; f64, mm (vcad
  convention). Every conic of revolution is a quadric, so intersection is
  a **closed-form quadratic** (Kahan-stable roots, near-vertex sheet
  selection, sag-branch consistency check) — there is no iteration and no
  discretization anywhere in the M0 trace.
- Refraction: vector-form Snell, exact in f64; the per-ray
  |n₁sinθ₁ − n₂sinθ₂| residual is carried as the exactness diagnostic
  (the analog of the particle crate's energy-drift column) and sits at
  ~1e-16 in practice.
- **Fail-closed ray fates:** every launched ray is imaged, vignetted (at
  a named surface), TIR'd (named surface), or missed — never silently
  dropped. Receipts refuse to price a bundle containing TIR/miss.
- Dispersion: three-term Sellmeier for N-BK7, F2, N-SF11, SF5 (Schott
  datasheet coefficients; the unit tests gate every glass's derived n_d
  and V_d against the catalog headline values so a transcription error
  fails loudly).
- Figures of merit: RMS spot radius over a field × wavelength grid on a
  deterministic equal-area pupil lattice (⟨ρ²⟩ = R²/2 exactly by
  construction), polychromatic pooled RMS, vignetting fraction, and the
  Airy radius 1.22·λ·N carried on every claim set as diffraction context.
- Paraxial y-u trace implemented **separately** (plus an independent
  2×2 ray-transfer-matrix path) — it is the analytic cross-check, not a
  convenience. EFL, BFD, Lagrange-invariant drift.
- Optimization: multi-start finite-difference minimization with
  scale-invariant stopping and a non-finite guard (infeasible designs are
  rejected, never averaged) — the M0 stand-in for the adjoint.
- Receipts: `vcad.optics-claims/1` (EFL, BFD, f/#, per-field poly RMS
  spot, chromatic focal shift, Airy radius) with full trace provenance;
  `design_claims` rides the unified `vcad.receipt/1` open domain
  vocabulary as `ClaimBasis::Predicted` → **rolls up Provisional, never
  Pass**.

**Out of scope at M0** (each a milestone below): no diffraction or
physical optics — **RMS spot is a geometric claim and says so on the
receipt**, with the Airy radius printed next to it; no aspheres (the
polynomial sag needs a Newton intersection); no paraxial pupil imaging
(the M0 pupil is the front-surface tangent plane — exact for
front-stop systems, honest-but-unweighted for internal stops); no
vignetting-aware field analysis; no tolerancing (that seam belongs to
`vcad-kernel-tolerance`); no adjoint yet.

**Freeze-the-discretization lesson, transplanted:** the pupil ray set is
a fixed deterministic lattice and the image plane follows the *paraxial*
BFD — a smooth analytic function of the parameters — so FD gradient
probes never see a re-gridded objective.

## Validation ladder (all in `cargo test -p vcad-kernel-optics`)

- Sellmeier n_d/V_d vs Schott catalog headline values for all four
  glasses; normal dispersion ordering.
- Plane surface at normal incidence: ray unchanged to 1e-15
  (mission-mandated exactness gate); Snell invariant < 1e-13 at steep
  incidence through dense flint.
- Paraxial trace vs the thick-lens closed forms
  1/f = (n−1)[c₁ − c₂ + (n−1)tc₁c₂/n] and BFD = f(1 − (n−1)tc₁/n)
  (Hecht §6.1) to 1e-9; thin-lens limit recovers the lensmaker's
  equation; y-u recurrence ≡ matrix path to 1e-9; Lagrange invariant
  conserved to 1e-12.
- Exact trace h → 0 limit equals the paraxial focus to 1e-6 mm, with
  h² convergence of the aberration.
- **Published prescription:** Thorlabs AC254-075-A (N-BK7/SF5,
  R 46.5/−33.9/−95.5, tc 7.0/2.5; [catalog data via
  3DOptix](https://www.3doptix.com/catalog/optics/lens/thorlabs/AC254-075-A),
  fetched 2026-07-17) traces to EFL 74.9 ± 0.4 mm and shows < 0.2 mm
  F→C shift — a falsifiable claim against an $80 part.
- **The U-curve:** exact-trace longitudinal spherical aberration of a
  bent singlet matches the Jenkins & White §9.5 third-order thin-lens
  formula within 8% across q ∈ [−2, 2] (sub-1% over most of the range),
  and the traced minimum lands at the textbook best-form
  q = 2(n²−1)/(n+2) ≈ 0.714 ± 0.08.
- Chromatic focal shift of a BK7 singlet matches the Abbe prediction
  f_C − f_F = f/V within 2%.
- The Dollond-condition doublet collapses chromatic shift > 10× vs the
  singlet; the published achromat beats the best-form singlet's
  polychromatic spot > 4×.
- Defocus blur equals the similar-triangles closed form δ·√⟨ρ²⟩/f to 2%
  (with the measured residual being the real third-order focus shift);
  spot collapses > 20× at the paraxial focus; every launched ray
  accounted for in the fate table.
- Optimizer: 1e-30-scale objectives don't false-converge (the particle
  crate's lesson, regression-tested here too); infeasible (+∞) regions
  never accepted; multi-start escapes a bad basin.

## The flagship: the optimizer rediscovers 1758

`cargo run --release -p vcad-kernel-optics --example achromat_design`

Multi-start FD optimization at f/5, EFL pinned to 100 mm, minimizing
polychromatic (F, d, C) RMS spot on axis:

| design | prescription found | poly RMS | chromatic shift |
|---|---|---:|---:|
| BK7 singlet (2 curvatures) | R 60.2 / −357.4 (q ≈ 0.71 — the best-form bending) | 79.8 µm | 1.534 mm (thin-lens f/V = 1.558) |
| BK7/F2 cemented doublet (3 curvatures) | R 48.2 / −41.1 / −347.8 | **8.14 µm** | **0.050 mm** |

- **9.8× spot improvement**, 31× chromatic collapse. Airy radius at f/5
  is 3.58 µm — the doublet is ~2.3× diffraction, honestly geometric.
- **The achromat condition emerges from raw ray tracing**: the optimized
  power split φ₁/φ = 2.329 vs the analytic Dollond ratio
  V₁/(V₁−V₂) = 2.308 — 0.9% deviation, with no chromatic theory anywhere
  in the objective. The residual deviation is the thin-lens
  approximation in the check, not noise: the optimizer trades a sliver
  of chroma against spherochromatism, exactly as real lens designers do.
- The singlet optimizer independently finds the best-form bending
  (q ≈ 0.71) — the U-curve minimum — from a cold start.
- Receipt: 6 claims under `vcad.optics-claims/1`, all
  `basis: predicted`, rolling up Provisional.

## Milestone ladder

- **M1 — aspheres + pupils + vignetting analysis.** Polynomial asphere
  sag with a Newton intersection (seeded by the conic closed form, with
  a fail-closed iteration cap); paraxial entrance-pupil imaging so
  internal-stop systems get a correctly weighted bundle; per-field
  vignetting curves; import of catalog prescriptions (Thorlabs publishes
  full data) as a fixture corpus.
- **M2 — the adjoint through the Snell chain.** Every M0 operation is
  smooth (quadratic root, Snell vector form, transfer) — reverse-mode
  d(RMS)/d(curvatures, thicknesses, indices) is *easier* than the
  particle crate's adjoint (no Dirichlet mask, no grid). Same
  optimizer-contract swap: `minimize_with_gradient` replaces FD without
  changing callers. FD validation at 0.1% or better, with the frozen
  ray-set rule making the comparison clean.
- **M3 — wavefront + field aberrations.** OPD fans, RMS wavefront error,
  Zernike decomposition; Seidel coma/astigmatism/field-curvature
  references join the ladder; field grids beyond on-axis become the
  default claim set.
- **M4 — the tolerance seam.** Radius/thickness/decenter sensitivities
  feed `vcad-kernel-tolerance` (WC/RSS/MC over the exact trace);
  as-built spot claims with stated yield.
- **M5 — MCP + BRep seam.** `design_lens` / `analyze_lens` tools; lens
  solids generated as revolved BRep for mounting geometry, tying the
  prescription to the CAD document that holds the barrel.

## Non-goals

No wave optics: interference, diffraction efficiency, coatings,
polarization, and beam propagation belong to `vcad-kernel-photonics`'s
regime (features ≈ λ) or to future physical-optics milestones. This
crate claims geometric spot sizes with the diffraction limit printed
alongside — never a resolution claim below the Airy radius.
