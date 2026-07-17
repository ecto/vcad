# An energy-audited, adjoint-ready voxel conduction solver inside a CAD kernel (draft)

Status: skeleton with current numbers; M0–M6 of `vcad-kernel-thermal`.

## Abstract

Every electronics enclosure, PSU, motor driver, and PCB ships against a
maximum-temperature requirement, and most of them are designed against a
hand rule. We describe a steady/transient heat-conduction solver embedded
in the vcad kernel next to the geometry it scores: finite volumes on a
uniform voxel grid with harmonic-mean face conductances, a matrix-free
Jacobi-PCG with a scale-invariant stopping criterion, per-axis diagonal
anisotropy, backward-Euler transients whose stored-vs-injected energy
identity is audited per run, and an adjoint that prices the gradient of a
smoothed peak temperature at one extra linear solve. Every solution
reports an energy balance; every claim it emits carries its grid, its
solver tolerance, and the film coefficient it was priced at — because in
natural convection, h *is* the error bar. Validation is exact where the
discretization is exact (layered composites at 200:1 contrast to 1e-14),
quantified where it is not (voxelized circles: first-order stair-step
error, measured), and checked against the JEDEC θ_ja band where only
consistency can be claimed.

## 1. Method

- **Discretization.** ∇·(k∇T) + q‴ = 0 integrated over voxels; face
  conductance G = A/(d/2k_P + d/2k_N) — the series half-cells, exact for
  layered 1D composites, conservative by construction. Boundary faces put
  the half-cell in series with the surface condition (Robin: + 1/h, so
  h → ∞ recovers Dirichlet smoothly). Anisotropy: per-axis k with the
  harmonic mean taken per axis component.
- **Solver.** Hand-rolled Jacobi-PCG, matrix-free stencil apply; stop at
  ‖r‖ ≤ tol·‖b‖ (scale-invariant). Fail-closed model validation:
  floating components (BFS over conductances), ghost sources, unresolvable
  θ references are errors, never defaults.
- **Transient.** Backward Euler, (C/Δt + A)Tⁿ⁺¹ = (C/Δt)Tⁿ + b, same PCG
  warm-started. The discrete identity ΣC·ΔT = Δt(P_src − P_out(Tⁿ⁺¹)) is
  audited per run (measured residual ~1e-10).
- **Adjoint.** A = Aᵀ, so dJ/dθ for J = p-norm-smoothed T_max costs one
  extra solve. Bracket max ≤ J−ref ≤ max·N_active^(1/p) reported.
  Parameters: per-region per-axis k, per-slot h, source powers. Geometry
  moves the discrete mask → finite differences, stated.

## 2. Validation ladder (measured)

| rung | closed form | result |
|---|---|---|
| Dirichlet slab | linear profile | exact at centers, ~1e-14 °C |
| Robin slab, h = 50…1e9 | T = q(1/h + (L−x)/k) | exact, < 1e-8 |
| composite slab, k 200:1 | q = ΔT/(L₁/k₁+L₂/k₂) | exact, 2.8e-14 °C |
| heated slab + film | T_max = T∞ + q((L−δ/2)/k + 1/h) | exact incl. δ/2 |
| cylinder shell | R = ln(r₂/r₁)/2πkH | stair-step error, halves 1 mm → 0.5 mm, < 5% |
| 3D chip-on-plate | energy balance | residual ~1e-11 |
| lumped capacitance | e^(−t/τ), Bi ≈ 1e-4 | < 0.5% of excess |
| semi-infinite solid | erfc(x/2√αt) | < 1.5% of step |
| anisotropic block | Q = k_axis·AΔT/L per axis | exact per axis |
| adjoint vs central FD | 6 parameters, 5 orders of scale | 2.1e-9 … 1.7e-6 rel |

## 3. Results

**hot_chip** (2 W, 10×10×1 mm die on 100×100×1.6 mm board, both faces
convecting, 130k voxels, ~800 CG iterations):

| board k (W/m·K) | h (W/m²K) | T_max (°C) | θ_ja (K/W) |
|---|---:|---:|---:|
| [15,15,15] | 5 | 67.45 | 21.23 |
| [15,15,15] | 10 | 56.48 | 15.74 |
| [15,15,15] | 30 | 46.92 | 10.96 |
| [15,15,0.5] | 10 | 69.91 | 22.45 |
| [0.3,0.3,0.3] | 10 | 359.14 | 167.07 |

The isotropic idealization hides 6.7 K/W (43% of θ); bare FR4 reads
359 °C — the copper planes are the heatsink, quantified. Step response
(anisotropic, h = 10): within 1 K of steady in ~380 s; energy audit 8e-11.

**Gradients** (two-material board, one adjoint solve): dJ/dP = 29.75 K/W,
dJ/dk in-plane = −23.7 K per W/m·K, dJ/dh ≈ −1.56/−1.58 K per W/m²K —
all FD-confirmed. The signs and magnitudes *are* the design guidance:
this board wants airflow and in-plane copper, in that order.

**Grid convergence** (hot_chip anisotropic): coarse pitches misrepresent
the die footprint (θ −5%/+13% jumps); once exact (≤ 1 mm) θ drifts ~1.3%
per further halving: 22.45 → 22.17 → 21.88 K/W. Quote with a ~2% grid
band.

**JEDEC-style consistency** (76.2×114.3 mm 2s2p-equivalent
[20, 20, 0.4], 9×9 mm 1 W die): θ_ja = 28.5 / 26.9 / 25.8 / 24.6 K/W at
h_eff = 8 / 10 / 12 / 15 — inside the published 20–30 K/W datasheet band
for this package class across all plausible still-air coefficients.
Consistency, not validation: h_eff bundles convection and radiation, and
the package (junction-to-board ~1–3 K/W) is absent.

## 4. Limitations (stated, always)

Conduction only. h supplied, never derived — natural-convection
correlations carry ±20–30%, and radiation at electronics temperatures is
the same order as natural convection (~6 W/m²K equivalent); treat h as
the combined coefficient or wait for the radiation milestone. Geometry
gradients are finite-difference (discrete mask). Voxel grids stair-step
curved geometry at first order (measured, §2).

## 5. Future work

Radiation exchange (the ~6 W/m²K elephant); joule heating imported from
the ecad PDN layer (trace/plane dissipation → `PowerSource`s — the
cross-domain loop this crate was aimed at); temperature-coefficient
adjoints (linear in b, unwired); shape adjoints on a frozen mask;
MaterialCard thermal properties from a phonon-conductivity atoms
milestone; `vcad-receipt` + MCP registration of `vcad.thermal-claims/1`.

## Reproduction

```
cargo test -p vcad-kernel-thermal
cargo run --release -p vcad-kernel-thermal --example hot_chip
cargo run --release -p vcad-kernel-thermal --example gradient_check
cargo run --release -p vcad-kernel-thermal --example convergence
cargo run    -p vcad-kernel-thermal --example composite_slab
```
