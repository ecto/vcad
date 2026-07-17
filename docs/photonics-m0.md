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

## M1 (landed): TF/SF mode injection + bend loss

The total-field/scattered-field plane injects the slab mode in one
direction: Ez at the first total-field column gets the missing incident
`hy_inc = −n_eff·P(y)·s(t + n_eff·Δ/2)` term, Hy at the last scattered
column subtracts `ez_inc = P(y)·s(t)` — the exact modal relation
Hy = −n_eff·Ez with the Yee half-cell/half-step offsets carried
explicitly. Honesty: the incident wave is the *continuum* mode with one
narrowband delay, so backward leakage is finite and **measured**:
−46.8 dB at pulse bandwidth f₀/4 (test asserts < −22 dB), forward
transmission 1.00000 between downstream monitors.

`Shape2::ring` (annular-sector polygon) builds bends; the 90° bend
example measures loss vs radius with the output arm on a *horizontal*
flux monitor (exercising the Sy path):

```
  R (units)   R/w     T = P_out/P_in   loss (dB)
  0.50         2.3    0.9053           0.432
  1.00         4.5    0.9847           0.067
  2.00         9.1    0.9964           0.016
  3.00        13.6    0.9978           0.010
```

(`cargo run --release -p vcad-kernel-photonics --example bend_loss`;
qualitative monotonicity is a debug-speed test, the table is the
release-mode example.) TF/SF is TM-only at M1 — the flagship splitter's
polarization; TE injection stays soft-source.

## M2 (landed): the discrete adjoint, FD-validated to 5×10⁻⁷

`adjoint::objective_and_gradient` returns dJ/dε at **every Ez sample in a
design region** for the mode-overlap objective J = |Σ w·Êz(ω)|², at the
cost of one extra FDTD run. The derivation rides the same identity as the
energy invariant: with `C_H = C_Eᵀ`, the transposed leapfrog —
time-reversed, with λ_E rescaled by ε — **is the forward stepper**, so
the adjoint pass is a second `Simulation` driven by soft sources on the
monitor line, and the gradient is the exact time-domain pairing
`dJ/dε_i = −Σ φ_i·ΔE_i` against stored forward increments (design-region
snapshots only).

Measured agreement with central differences (frozen step count across
forward, adjoint, and every probe): **4.8×10⁻⁷ / 4.6×10⁻⁶ / 1.6×10⁻⁶**
relative at core / core-edge / cladding probe cells, with the FD referee
h-stable to five digits.

Two failures that taught (now impossible by assertion):

1. **A monitor line that dips into the CPML poisons the adjoint.** The
   forward objective is fine, but the adjoint injects sources on those
   rows, where untransposed-ψ dynamics are first-order wrong — a 17 %
   gradient error from two rows of overlap. `validate_geometry` now
   rejects monitors that touch the slabs (and design regions, and
   forward sources overlapping the region).
2. **Adjoint bookkeeping errors hide in near-cancellation.** The
   source-phase index and the pairing index were each off by one — and
   *compensating*, producing a plausible-looking 13 % error; fixing only
   one blew the gradient up 2500×. The discriminating experiment was a
   closed PEC box, where the transposition is provably exact: it isolated
   the derivation (exact) from the geometry traps (the real bugs).

The untransposed-CPML approximation (the adjoint reuses the same
absorber) prices in at the measured reflection floor: ~4×10⁻¹⁰ absolute
on the gradient in the discrimination experiment — negligible, and now
stated rather than assumed.

## M3 (landed): topology parameterization + the spec seam

`design::TopologyParam`: density ρ ∈ [0,1] per design cell → **cone
filter** (radius = minimum feature scale, border-clipped normalization so
constants survive corners) → **smoothed-Heaviside projection** (β = 0 is
the identity, β → ∞ binarizes; the optimizer owns the β ramp — the
binarization schedule) → linear ε interpolation. The chain rule runs
backward through the **exact filter transpose** (`⟨Fρ,g⟩ = ⟨ρ,Fᵀg⟩`
unit-tested to 10⁻¹²) and the analytic projection derivative.

End-to-end validation: dJ/dρ through
density → filter → project → ε → FDTD → mode overlap, adjoint + chain
rule vs central differences on raw densities:
**1.3×10⁻⁷ / 5.4×10⁻⁷ / 2.5×10⁻⁷** relative at three probe components.

`spec::TopologyProblemSpec` (`vcad.photonics-spec/1`) is the serde seam:
every scalar knob is a `ParamValue` (literal or **named** document
parameter), resolution is fail-closed (unbound name ⇒ error, never a
default; NaN ⇒ error; densities validated against the region, ρ ∉ [0,1]
rejected). The density vector travels as data; β is schedule state, not
a document parameter. JSON round-trip tested.

## M4 (landed): `vcad.photonics-claims/1`

`receipt::splitter_claims` — per-arm transmission, insertion loss,
splitting ratio, arm dB levels, `min_feature_nm` (honestly labeled a
regularization scale) — fail-closed (empty spectrum, non-positive/NaN
powers, missing center frequency all refuse), with provenance that
prices the solver's own dispersion error and the cells/λ **in the core
material** into every claim set. `basis: "predicted"` throughout.
(vcad-receipt/MCP wiring is the flagged cross-crate follow-up.)

## M5 (landed): THE flagship — the inverse-designed 1×2 splitter

`cargo run --release -p vcad-kernel-photonics --example splitter_inverse_design`

2×2 design box (2704 density cells) between an input guide and two
output arms; per-arm adjoint gradients (one forward + two adjoint runs
per iteration, shared forward pass); Y-taper seed; β schedule 4→128;
195 FDTD runs. **Hard-thresholded before claiming** — the binarization
gap came out at **0.02 %** (gray FoM 70.305 → binary 70.291), i.e. the
design is genuinely two-phase. Claims at λ₀ = 1.55:

| claim | value |
|---|---|
| transmission arm A / arm B | 0.4938 / 0.5000 |
| per-arm level | **3.064 dB / 3.011 dB** (target 3.01 dB) |
| splitting ratio | 0.4969 |
| insertion loss | 0.027 dB |
| reflection (phasor-subtraction) | 2×10⁻⁴ |
| min feature | 232.5 nm (cone-filter diameter) |

Broadband: total transmission 0.992–0.995 and ratio 0.497–0.498 across
λ = 1.50–1.60. GDS: 47 rectangles, µm/nm units, exact pixel geometry.
Convergence of the final binary geometry (pixel shapes re-painted):

```
  res      T_a      T_b      total
  λ/40    0.4938   0.5000   0.9938   (native — matches characterization)
  λ/60    0.4857   0.4878   0.9735
  λ/80    0.4857   0.4876   0.9733   (converged: IL ≈ 0.12 dB)
```

The native-grid claim is ~2 % optimistic on total transmission; the
λ/60→λ/80 drift is 2×10⁻⁴ — the converged-grid insertion loss of the
shipped geometry is **0.12 dB**, and that is the number to quote against
hardware. Three optimization-campaign lessons are recorded in the paper
draft (source back-action vs reference normalization; fixed-window
resonance exploitation; claim-the-binary-twin).

### Meep benchmark configuration

`examples/meep_bend_benchmark.rs` reproduces Meep's published
`bend-flux` setup verbatim (ε = 12, w = 1, 16×32 cell, res 10, PML 1,
fcen 0.15, df 0.1, Ez ≡ our TM), with reflection via our port of
`load_minus_flux` (identical-run subtraction residual: 10⁻³⁰). The Meep
docs publish curves, not numbers, so the quantitative A/B is: run the
matching script below and diff the tables.

```python
# pip install meep;  python bend_ab.py   — mirrors our example exactly
import meep as mp
sx, sy, w, pad, dpml, res = 16, 32, 1, 4, 1.0, 10
ycen, xcen = -0.5*(sy-w-2*pad), 0.5*(sx-w-2*pad)
fcen, df, nfreq = 0.15, 0.1, 11
def run(bend):
    geom = ([mp.Block(mp.Vector3(sx-pad, w, mp.inf), center=mp.Vector3(-0.5*pad, ycen),
                      material=mp.Medium(epsilon=12)),
             mp.Block(mp.Vector3(w, sy-pad, mp.inf), center=mp.Vector3(xcen, 0.5*pad),
                      material=mp.Medium(epsilon=12))] if bend else
            [mp.Block(mp.Vector3(mp.inf, w, mp.inf), center=mp.Vector3(0, ycen),
                      material=mp.Medium(epsilon=12))])
    sim = mp.Simulation(cell_size=mp.Vector3(sx, sy), resolution=res,
                        boundary_layers=[mp.PML(dpml)], geometry=geom,
                        sources=[mp.Source(mp.GaussianSource(fcen, fwidth=df), mp.Ez,
                                 center=mp.Vector3(-0.5*sx+dpml, ycen), size=mp.Vector3(0, w))])
    refl = sim.add_flux(fcen, df, nfreq, mp.FluxRegion(
        center=mp.Vector3(-0.5*sx+dpml+0.5, ycen), size=mp.Vector3(0, 2*w)))
    tran_region = (mp.FluxRegion(center=mp.Vector3(xcen, 0.5*sy-dpml-0.5), size=mp.Vector3(2*w, 0))
                   if bend else
                   mp.FluxRegion(center=mp.Vector3(0.5*sx-dpml, ycen), size=mp.Vector3(0, 2*w)))
    tran = sim.add_flux(fcen, df, nfreq, tran_region)
    return sim, refl, tran
sim, refl, tran = run(False)
sim.run(until_after_sources=mp.stop_when_fields_decayed(
    50, mp.Ez, mp.Vector3(0.5*sx-dpml-0.5, ycen), 1e-3))
straight_refl_data, straight_tran = sim.get_flux_data(refl), mp.get_fluxes(tran)
sim, refl, tran = run(True)
sim.load_minus_flux_data(refl, straight_refl_data)
sim.run(until_after_sources=mp.stop_when_fields_decayed(
    50, mp.Ez, mp.Vector3(xcen, 0.5*sy-dpml-0.5), 1e-3))
for f, r, t, p0 in zip(mp.get_flux_freqs(refl), mp.get_fluxes(refl),
                       mp.get_fluxes(tran), straight_tran):
    print(f"lambda {1/f:.3f}  R {-r/p0:.5f}  T {t/p0:.5f}  loss {1+r/p0-t/p0:.5f}")
```

Our table for the same configuration (from the example; loss is the
closure 1 − R − T, all values positive — energy-sane):

```
  λ         f       R         T         loss
  10.000    0.1000  0.22103   0.46703   0.31194
  8.333     0.1200  0.25224   0.28387   0.46390
  6.667     0.1500  0.23235   0.12231   0.64535
  5.556     0.1800  0.24787   0.35451   0.39762
  5.000     0.2000  0.27747   0.36719   0.35534
```

(A sharp 90° corner in a λ/w ≈ 5–10 guide reflects and radiates hard —
the transmission dip near mid-band is the corner anti-resonance.)

## M6 (landed): tape-out pack

`docs/photonics-tapeout.md` — what the GDS is (exact pixel geometry,
rect decomposition, nm grid), the e-beam shuttle design-rule checklist,
the 2D→3D honesty clause, and `receipt::compare` — mechanical
Holds/Violated/Unmeasured verdicts binding lab measurements to claims
(`Unmeasured` is never assumed to hold; NaN measurements are
violations).

## Milestone ladder (next)

- Effective-index 3D→2D reduction (prices the 2D→3D gap into claims).
- TE adjoint + injection; anisotropic sub-pixel smoothing.
- Broadband multi-frequency objectives (kills window-resonance
  exploitation at the root).
- `crates/vcad-receipt` + MCP wiring for the claims family
  (cross-crate schema + TS codegen — flagged follow-up PR).
