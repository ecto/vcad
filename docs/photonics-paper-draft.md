# Paper draft: A receipt-native inverse-design pipeline for planar photonics

*Skeleton + verified numbers. Sections marked ▢ get prose when the
milestone data is final; every number cited is produced by a shipped
test or example in `vcad-kernel-photonics`.*

## Abstract (draft)

We present a 2D FDTD electromagnetics kernel with discrete-adjoint
inverse design that ships fab-ready GDS, built inside a parametric CAD
system rather than beside one. The solver validates against exact
*discrete* closed forms (its own dispersion relation to 5×10⁻⁸; the
discrete two-media reflectance to 10⁻⁴), conserves a discrete energy
invariant to 10⁻¹¹, and its adjoint gradients match finite differences
to 5×10⁻⁷ — with the two failure modes we found promoted to
assertions. A density-based topology optimization with exact-transpose
filtering and a fail-closed serialization seam produces a 1×2 power
splitter whose binarized pixel geometry — the exact geometry simulated —
exports to GDS with predicted-performance claims carrying full solver
provenance, and whose measurement plan is a mechanical
Holds/Violated/Unmeasured comparison. The pipeline's distinguishing
property is not speed but *accountability*: every shipped number knows
how it was produced and what would falsify it.

## 1. Introduction

- Inverse design is the canonical adjoint showcase (Molesky et al.,
  Nat. Photonics 12, 659 (2018)); incumbents: Lumerical (closed),
  Meep (open, respected; Oskooi et al., CPC 181, 687 (2010)) — neither
  CAD-native nor receipt-native.
- Thesis: the design loop should end in an *artifact with claims*, not
  a figure. CAD kernel residency gives geometry, export, and claim
  plumbing for free.
- ▢ positioning vs SPINS/Ceviche (autograd FDFD) — our contribution is
  the discipline stack, not a new discretization.

## 2. Methods

### 2.1 Solver
Yee grid, TM/TE, f64, leapfrog; normalized units c = ε₀ = μ₀ = 1
(f = 1/λ). CPML (Roden–Gedney) with per-sample staggered-depth
coefficients. PEC/PMC walls; PMC turns y-uniform TM into exact 1D — the
validation instrument. Scalar area-weighted sub-pixel ε averaging
(anisotropic smoothing: future work, flagged).

### 2.2 The discrete-first validation ladder
Numbers from `cargo test -p vcad-kernel-photonics` (Table 1):

| Rung | Reference | Result |
|---|---|---|
| Dispersion (20 c/λ) | exact discrete relation | 5×10⁻⁸ rel (continuum 3.6×10⁻³ away) |
| Fresnel 1→2 | exact discrete R_d = sin²((k₁−k₂)Δ/2)/sin²((k₁+k₂)Δ/2) | ~1×10⁻⁴; O(Δ²) → 1/9; TM/TE duality <10⁻⁵ |
| CPML 12 cells | < −50 dB req. | −95.6 dB measured |
| Energy (PEC box) | exact invariant ½(⟨εEⁿ⁺¹,Eⁿ⟩+\|Hⁿ⁺½\|²) | <10⁻¹¹ drift |
| Slab n_eff | transcendental 2.85136 | FDTD 2.84984 (0.05 %) |
| TF/SF directivity | — | −46.8 dB backward |
| Bend loss | monotone in R | 0.43→0.01 dB (R 0.5→3) |

The design rule behind the ladder: validate against the discretization's
*own* closed forms at tight tolerance, then show O(Δ²) convergence to
the continuum — never launder discretization error through a loose
tolerance against the continuum formula.

### 2.3 The adjoint
One leapfrog step u^{n+1} = M·u^n + s; the gradient chain runs Mᵀ
backward. With C_H = C_Eᵀ (the energy-invariant identity), the
transform φ = ε⁻¹λ_E, Ψ = −λ_H, m = N−n makes MᵀT⁻¹ ≡ the forward
stepper: the adjoint is a second FDTD run with monitor-line sources;
dJ/dε_i = −Σ φ_i·ΔE_i exactly. FD validation: 4.8×10⁻⁷ (core),
4.6×10⁻⁶, 1.6×10⁻⁶.

Two taught failures, now assertions: (i) monitor rows inside the CPML
poison the adjoint injection (17 % error from two rows); (ii)
compensating off-by-ones in source phase and pairing hid inside
near-cancellation — isolated by a closed-PEC-box experiment where the
transposition is provably exact.

### 2.4 Topology parameterization and optimizer
ρ → cone filter (min feature; exact transpose, ⟨Fρ,g⟩=⟨ρ,Fᵀg⟩ to
10⁻¹²) → tanh projection (β schedule) → linear ε. End-to-end dJ/dρ vs
FD: 1.3–5.4×10⁻⁷. Projected gradient ascent, ‖·‖∞-normalized steps,
monotone acceptance, β ∈ {4…128}.

### 2.5 Claims and the seam
`vcad.photonics-spec/1` (fail-closed named parameters; densities as
data) and `vcad.photonics-claims/1` (per-arm T, insertion loss,
splitting ratio, min feature; provenance includes cells/λ *in the
core*, run time, CPML, and the solver's own dispersion error).
`compare()` renders Holds/Violated/Unmeasured verdicts; unmeasured is
never assumed.

## 3. The flagship: 1×2 splitter

Final numbers from `examples/splitter_inverse_design.rs`: 2×2 design
box, 2704 density cells, Y-taper seed, per-arm adjoint (1 forward + 2
adjoint runs per iteration, shared forward), β schedule 4→128, 195 FDTD
runs, hard-thresholded before claiming (binarization gap **0.02 %**).
At λ₀ = 1.55: per-arm **3.064 / 3.011 dB** against the 3.01 dB perfect-
split target, splitting ratio 0.4969, insertion loss 0.027 dB (native
grid) / **0.12 dB at the converged grid** (λ/60→λ/80 drift 2×10⁻⁴),
reflection 2×10⁻⁴, ratio flat at 0.497–0.498 over λ = 1.50–1.60. GDS:
47 rectangles, 232.5 nm minimum feature.

Lessons already banked from the first optimization campaigns:

1. **Reference-run normalization breaks under source back-action** when
   the design box is near the TF/SF plane (measured T > 1); same-run
   net-input normalization restores the energy bound.
2. **Fixed-window objectives breed resonance exploitation**: the
   optimizer parks energy in slow states that ring through the monitor
   after the window; apparent flux imbalance at exactly f₀ was the
   tell. Characterization windows must cover the ring-down.
3. **Claims must be made on the binarized twin** — gray boundary cells
   do real optical work (measured 6.7 % FoM gap), and the fab receives
   the binary geometry.

## 4. Benchmark against Meep's published bend

Meep's `bend-flux` configuration reproduced verbatim (ε = 12, w = 1,
16×32 cell, res 10, PML 1, fcen 0.15, df 0.1), reflection via our port
of `load_minus_flux` (validated: identical-run subtraction residual
10⁻³⁰). The Meep docs publish curves, not numbers; the A/B is
mechanical — the matching 25-line Meep script ships in the docs. ▢
paste both tables when a Meep run is available.

## 5. Convergence and limits

Splitter convergence: total T 0.9938 (λ/40, native) → 0.9735 (λ/60) →
0.9733 (λ/80): the native claim is ~2 % optimistic and the converged
number is stable to 2×10⁻⁴. Stated limits: 2D (chip predictions are qualitative — the tape-out doc
carries the measurement plan that prices the 2D→3D gap); linear
lossless non-dispersive ε; scalar sub-pixel smoothing; TM-only
injection/adjoint.

## References

▢ Taflove & Hagness 3rd ed.; Roden & Gedney (2000); Oskooi et al.
(2010); Farjadpour et al. (2006); Molesky et al. (2018); Hughes et al.
(2018); SiEPIC ebeam PDK.
