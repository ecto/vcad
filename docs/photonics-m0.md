# Photonics M0: 2D FDTD with a validation ladder that names its own floors

`vcad-kernel-photonics` makes vcad a design tool for planar photonics:
waveguides, splitters, ring resonators, grating couplers — the component
library of silicon photonics. The destination is **inverse design**: adjoint
gradients of transmission with respect to every design cell's permittivity,
density-based topology optimization under fabrication constraints, and
fab-ready GDS out the other end (via `vcad-gdsii`). Inverse-designed
photonics is the canonical adjoint-method showcase; the incumbents are
Lumerical (closed, expensive) and Meep (respected, but neither CAD-native
nor receipt-native). This crate is the M0 of that ladder, built to the
discipline established by `vcad-kernel-particle`: solver → analytic
validation → adjoint (FD-validated) → parameter seam (fail-closed) →
receipt claims with provenance.

## M0 scope (and honesty)

**In scope:** the forward solver, absorbing boundaries, eigenmode sources,
spectral monitors, and a validation ladder.

- 2D FDTD on the Yee grid, TM (Ez, Hx, Hy) and TE (Hz, Ex, Ey)
  polarizations, `f64` fields, leapfrog in time. Naming follows
  Joannopoulos/Meep — **TM = E out of plane** — with the slab-literature
  cross-map documented where it bites (`modes.rs`).
- Normalized units, stated loudly: c = ε₀ = μ₀ = 1, one length unit ≡ 1 µm
  by convention, **f = 1/λ**. Nothing in the solver knows meters.
- CPML absorbing boundaries (Roden–Gedney recursive convolution, graded
  polynomial σ, CFS α/κ knobs present, per-side thicknesses), evaluated at
  each staggered sample's true half-cell depth.
- Materials: isotropic ε(x, y) painted from rects / circles / polygons with
  area-weighted sub-pixel averaging (4×4 supersampling). Scalar smoothing
  only — the anisotropic-tensor scheme (Farjadpour et al. 2006) that
  restores O(Δ²) at arbitrary interfaces is a later milestone.
- Sources: soft (additive) point and line sources; the line source carries
  the slab eigenmode profile from the built-in **bisection mode solver**
  (the transcendental equation `v = s·u·tan u`, `u² + v² = V²` — monotone
  on the fundamental branch, so bisection cannot fail).
- Monitors: running-DFT field monitors on lines, reduced to Poynting flux
  with the Yee half-step time phases carried exactly; point time-probes.
- Walls: PEC or PMC per side. PMC y-walls make a y-uniform TM wave
  **exactly 1D** (PEC does the same for TE by duality) — several
  validation rungs run in this configuration, where tolerances are set by
  physics rather than by diffraction contamination.

**Out of scope at M0** (each is a milestone below): TF/SF unidirectional
injection, the adjoint, topology parameterization, material dispersion
(single ε per material — right at one design wavelength, wrong for
broadband material physics), loss/gain, 3D, effective-index reduction of
3D stacks. Do not read foundry-grade predictions out of a 2D solver: 2D
answers are exact for the 2D problem and *qualitative* for real chips.

## Validation ladder (all in `cargo test -p vcad-kernel-photonics`)

Measured values from the shipped tests (release, same numbers in debug):

| Rung | Reference | Measured |
|---|---|---|
| Numerical dispersion, 20 cells/λ, S = 0.5 | discrete relation `sin(ωdt/2)/dt = sin(kΔ/2)/Δ` (Taflove ch. 4) | k matches to **5×10⁻⁸** rel; continuum k is 3.6×10⁻³ away — the solver reproduces its own error model, not a loose tolerance |
| Fresnel, n 1→2 half-space, TM & TE, 3 freqs | exact **discrete** reflectance `R_d = sin²((k₁−k₂)Δ/2)/sin²((k₁+k₂)Δ/2)` (derived by matching the discrete Helmholtz recurrence; → Fresnel as O(Δ²)) | matches R_d to ~1×10⁻⁴; R+T = 1 to 1×10⁻⁵; res 20→40 shrinks the continuum error 4.2× (O(Δ²) confirmed); TM/TE agree to <1×10⁻⁵ (exact 1D duality) |
| CPML reflection, 12 cells, m = 3, σ_scale 0.8 | < −50 dB required | **−95.6 dB** measured (reference-subtraction method) |
| Energy in a lossless PEC box (both pols, inhomogeneous ε) | discrete invariant `U = ½(⟨εEⁿ⁺¹,Eⁿ⟩ + |Hⁿ⁺½|²)` — exactly conserved because the staggered curls are mutually adjoint under PEC | drift < 10⁻¹¹ relative over 400 steps (rounding only) |
| Slab waveguide, n 3.48/1.44, w 0.22, λ 1.55 | transcendental n_eff = 2.85136 | FDTD propagation phase gives 2.84984 (**0.05 %**); T(straight guide) = 0.997 across the band; waveguide-dispersion sign correct |

Two lessons the ladder taught (kept as tests):

1. **Interface placement is physics.** A dielectric interface aligned with
   a sample line gets area-averaged to (ε₁+ε₂)/2 — a one-cell
   antireflection layer that lowers R by ~8 % at 20 cells/λ. On a
   half-cell line, no ε sample straddles the boundary and the discrete
   problem is sharp. The Fresnel rung pins the sharp case to its exact
   discrete closed form; the smeared case is sub-pixel averaging doing its
   job (and converges to the same continuum limit).
2. **The Hz lattice cannot center on an even grid.** TE mirror-symmetry
   tests need odd cell counts — with an even grid the walls sit 20.5Δ vs
   19.5Δ from a source on the half-lattice and their echoes break symmetry
   at the wavefront amplitude. The 4-fold symmetry tests (TM even grid,
   TE odd grid) hold at 10⁻¹³·max.

## The benchmark table

`cargo run --release -p vcad-kernel-photonics --example waveguide_transmission`

```
slab mode @ λ = 1.55: n_eff = 2.851356 (V = 1.4127, residual 8.9e-16)
grid 170×70 @ Δ = λ₀/40, Courant 0.5, 3200 steps, t = 43.8

  λ        f        n_eff(theory)  n_eff(FDTD)  T = P_out/P_in
  1.45     0.6897   2.8990         2.9026       0.9979
  1.50     0.6667   2.8750         2.8755       0.9978
  1.55     0.6452   2.8514         2.8491       0.9977
  1.60     0.6250   2.8279         2.8233       0.9975
  1.65     0.6061   2.8048         2.7981       0.9974
```

## Design decisions that encode the particle-optics lessons

- **A `Simulation` is single-shot.** The first step freezes configuration;
  parameter studies rebuild. This makes frozen-discretization comparisons
  (finite differences at M2, line searches at M5) the default, not a
  discipline the caller must remember — the exact lesson
  `vcad-kernel-particle` paid for in its adjoint milestone.
- **The error model is exported.** `dispersion::fdtd_wavenumber[_in_medium]`
  is public API: tests, receipts, and users can price the gap between grid
  physics and continuum physics instead of discovering it.
- **Gated waveforms.** The Gaussian source is hard-gated to exactly zero
  after `cutoff·σ`, so post-source invariants (energy conservation, the
  adjoint's reverse pass at M2) have a definite, reproducible support.
- **Fail-closed guards.** NaN geometry is rejected in the mode solver
  (guards written as "not provably valid", not "invalid"); permittivity
  must be ≥ 1; monitors and sources validate their staggered index ranges
  at construction.

## Milestone ladder

- **M1 — TF/SF + bends:** total-field/scattered-field unidirectional mode
  injection; waveguide bend with measured bend loss vs radius.
- **M2 — the adjoint:** reverse-time run with adjoint sources at the
  objective monitor; ∂T/∂ε per design cell; validated against central
  differences on perturbed cells with frozen run length (linear wave
  physics should be kinder than particle chaos was — verify, don't assume).
- **M3 — topology parameterization:** density field → filter (minimum
  feature) → projection (binarization schedule) → ε; serde design spec
  with named parameters, fail-closed resolution.
- **M4 — claims:** `vcad.photonics-claims/1` — transmission,
  insertion_loss_db, splitting_ratio, min_feature_nm — with grid,
  cells/λ, run-time, and monitor-window provenance.
- **M5 — the flagship:** inverse-designed 1×2 splitter (50/50 target,
  −3.01 dB per arm), GDS export via `vcad-gdsii`, forward-solver benchmark
  against a published Meep result, convergence study, paper-draft skeleton.
- **M6 — tape-out pack:** design-rule notes for e-beam/shuttle runs,
  min-feature honesty, and a `compare()` measurement schema for when a
  chip comes back.
