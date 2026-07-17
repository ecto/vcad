# Heat-conduction FEA M1: transient response and anisotropic conductivity

Second rung of the `vcad-kernel-thermal` ladder (M0: `docs/thermal-m0.md`).
Two capabilities, each with its own conscience.

## Transient: backward Euler with an energy identity

`transient::solve_transient` steps `(C/Δt + A)·Tⁿ⁺¹ = (C/Δt)·Tⁿ + b` with
per-voxel thermal mass C = ρc_p·V. Backward Euler because conduction is
stiff — the stable explicit step scales with Δx² and would be thousands of
steps per second on fine grids; implicit stepping is unconditionally stable
and lets Δt follow the physics. The cost is first-order accuracy in Δt,
stated: halve the step to check convergence in time, the same way you halve
the grid in space. Each step reuses the steady PCG (same operator plus a
positive diagonal), warm-started from the previous step.

**The audit:** summing the discrete update over free voxels cancels every
internal face (the scheme is conservative), leaving the *identity*

> ΣC·(Tⁿ⁺¹ − Tⁿ) = Δt·(P_source − P_out(Tⁿ⁺¹))

so stored-energy change must equal net injected energy to CG tolerance —
not approximately, but as a property of the discretization. Every
`TransientSolution` reports `energy_audit_residual_rel` (~1e-10 in
practice); the final state's steady balance residual doubles as a
distance-from-steady meter.

Heat capacity is fail-closed: a transient solve on any material region that
didn't declare ρc_p errors with the region index, never defaults.

## Anisotropy: per-axis diagonal conductivity

`MaterialRegion` now carries `k_w_mk: [f64; 3]` — the diagonal-tensor case
that boards actually are (copper planes: in-plane ~15–20 W/m·K,
through-plane ~0.3–0.5). Face conductances take the harmonic mean of the
*axis component* on each side. Off-diagonal tensors (rotated laminates) are
out of scope and stated.

## Validation (all in `cargo test -p vcad-kernel-thermal`)

- **Lumped capacitance** (Incropera ch. 5): 10 mm aluminum cube, Bi ≈ 1e-4,
  h = 20 from 100 °C into 25 °C. Exact τ = ρcV/(hA) = 202.6 s; backward
  Euler at Δt = τ/200 tracks the exponential to < 0.5% of the initial
  excess at t = τ/2, τ, and 2τ. Energy audit < 1e-6.
- **Semi-infinite solid** (Carslaw & Jaeger): face stepped to 100 °C,
  T(x,t) = T_s·erfc(x/2√(αt)). At t = 100 s the computed profile matches
  erfc at η = 0.26, 0.51, 1.02 to < 1.5% of the step. The 50 mm domain's
  far end sits at η = 2.5 (erfc ≈ 4e-4) — still semi-infinite, checked.
- **Relaxation to steady:** the chip-on-plate run lands on the
  `solve_steady` field to < 0.01 K after ~16 time constants, approaches
  monotonically from below, and its final balance residual < 1e-3.
- **Per-axis Fourier:** a k = [20, 5, 0.5] block driven axis by axis
  reproduces Q = k_axis·A·ΔT/L independently per axis to solver tolerance
  (the discretization error is exactly zero for linear profiles; what
  remains is CG's 1e-8).

## hot_chip, upgraded (`--release --example hot_chip`)

| board k (W/m·K) | T_max (°C) | θ_ja (K/W) |
|---|---:|---:|
| isotropic [15, 15, 15] | 56.48 | 15.74 |
| real-ish [15, 15, 0.5] | 69.91 | 22.45 |
| bare FR4 [0.3, 0.3, 0.3] | 359.14 | 167.07 |

Findings:

1. **The isotropic idealization was hiding 6.7 K/W.** M0's honesty box
   guessed the error was "modest"; the measurement says +13.4 K on a 31.5 K
   rise — a 43% θ error. The box has been corrected. Lesson: don't
   adjective an error you can compute.
2. **Bare FR4 reads 359 °C** — the quantitative version of "this is why
   copper planes are your heatsink". The spread between rows two and three
   is the entire thermal value of the copper in a 4-layer board.
3. **Step response:** with the real-ish board at h = 10, power-on from a
   25 °C soak reaches within 1 K of steady in ~380 s (die front runs ahead
   of the board: 41 °C after the first 5 s step while the plate is still
   cold). Energy audit residual 8e-11 over 240 steps.

All previous caveats stand: h supplied not derived, no radiation,
conduction only.
