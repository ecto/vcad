# Differentiable seam — M10 design note

M0–M9 built the first-order seam: `dx/dθ` (forward and reverse), mass-property
QoIs, seeding synthesis, and an L-BFGS optimizer that prices every parameter
from one adjoint pass. M10 is the last milestone of the wave — **second-order
derivatives** where they are cheap and honest, and **performance**, guided by
measurement rather than assumption.

The governing rule of this milestone: *every second-order object states
exactly what it computes and what it drops.* Nothing here silently ships a
"full Hessian" with a missing term.

## What landed

### Part 1 — second order

1. **Gauss–Newton curvature** (`gauss_newton.rs`:
   `gauss_newton_hessian`, `gauss_newton_hvp`, `gauss_newton_gradient`).
2. **Exact directional `d²V/dθ²`** for the volume QoI via second-order node
   kinematics (`second_order.rs`: `evaluate_with_second_derivative`,
   `volume_with_second_derivative`, `SecondOrderSeeding`, `SeamMeshSecond`;
   `implicit.rs`: `constraint_row_2`).

### Part 2 — performance

3. Grid-accelerated evaluation-time vertex resolution (`frozen.rs`:
   `PointGrid`, `evaluate_plan` / `evaluate_plan_naive`), after profiling
   redirected the effort from capture (already hashed / not a bottleneck) to
   the one genuinely `O(nodes·vertices)` scan.

---

## Part 1.1 — Gauss–Newton curvature

For a least-squares objective `J(θ) = Σ_q r_q(θ)²` — the shape *every*
mass-property recovery objective in this crate takes (`VolumeMatch`, the M9
five-QoI `QoiMatch`, each residual a relative miss `r_q = (Q_q − t_q)/t_q`) —
the exact gradient and Hessian are

```text
g = 2 Jᵀ r
H = 2 JᵀJ  +  2 Σ_q r_q ∇²r_q ,     J = ∂r/∂θ  (m residuals × n params)
```

**What `gauss_newton_hessian` / `gauss_newton_hvp` compute:** exactly the first
term, `H_GN = 2 JᵀJ`, and products with it. They **drop** the residual-
curvature term `2 Σ_q r_q ∇²r_q`. That term needs the second derivatives of
each QoI (node accelerations); the Gauss–Newton term needs only the residual
Jacobian `J`, which the seam already prices exactly (forward: one seam pass per
parameter; reverse: one pullback per residual QoI). So `H_GN` is **exact at
zero residual** and `O(‖r‖)`-accurate near it — the trusted curvature model in
the neighbourhood of an optimum, and a positive-semidefinite lower model
everywhere. `gauss_newton_hvp` forms `H_GN·v = 2 Jᵀ(Jv)` matrix-free (`O(m·n)`)
for a truncated-Newton inner loop; `gauss_newton_gradient` returns the
*exact* `g = 2 Jᵀr` so the factor-of-two bookkeeping lives in one place.

**Gate** (`m10_second_order.rs::m10_gauss_newton_hessian_matches_full_hessian_near_optimum`):
on the M9 five-parameter model at θ* = `[10, 12, 8, 1.8, 2.2]`, the QoI Jacobian
is built from the trusted forward-mode `mass_properties_with_derivative`, and
`H_GN = 2 JᵀJ` is compared against a central finite difference of the *exact*
gradient `g(θ) = 2 Jᵀr` (= the full Hessian). Near θ* the residuals vanish, so
the dropped term is ≈ 0 and `H_full ≈ H_GN`; the fillet model's re-capture
correspondence noise (the M9 fillet-frame caveat) dominates the FD Hessian, so
the gate is loose and aggregate. Measured (FD step `h = 1e-3`):

| Quantity | GN | full (FD) |
|---|---|---|
| eigenvalues | `[4.97e-4, 1.08e-2, 1.69e-2, 3.15e-2, 4.116e-1]` | `[4.96e-4, 1.09e-2, 1.70e-2, 3.16e-2, 4.130e-1]` |
| trace | `0.47132` | `0.47288` (rel **3.3e-3**) |
| ‖H_GN − H_full‖_F / ‖H_GN‖_F | — | **4.6e-3** |

`H_GN` is PSD by construction; the whole spectrum matches the full Hessian to
~0.3–0.5% near the optimum, i.e. the residual term is negligible there exactly
as the theory says.

## Part 1.2 — exact `d²V/dθ²` for the volume QoI

The honest second derivative of the volume along one parameter is

```text
d²V/dθ² = Σ_ij (∂²V/∂x_i∂x_j) ẋ_i ẋ_j  +  Σ_i (∂V/∂x_i) ẍ_i,
```

both terms carried in full. The whole computation reduces to one pass of the
shared generic integral `mesh_volume` over the nested scalar
`Dual<Dual<f64>>`: seed each node coordinate as `((x, ẋ), (ẋ, ẍ))` and read
`(V, dV/dθ, d²V/dθ²)` off the value / first-tangent / second-tangent slots.
`Dual<Dual<f64>>` satisfies `tang::Scalar` (every `Dual<S: Scalar>` does — probed
by `second_order::tests::nested_dual_recovers_second_derivative`), so
`mesh_volume` compiles at that type unchanged. This genericity was always the
second-order on-ramp; M10 walks up it.

The work is the node accelerations `ẍ_i`:

- **Lift nodes** (`SurfaceUv`) on plane/cylinder/sphere: the surface point
  `x(θ) = S(u, v; fields(θ))` is **linear** in the seeded fields (translation,
  radius) at a frozen `(u, v)`, so `∂x/∂field` is a constant map and
  `ẍ = (∂x/∂field)·field̈` — exactly the first-order lift evaluated with the
  field *accelerations* seeded where velocities go. No `Dual<Dual>` lift is
  needed for these kinds. Cone/torus points are nonlinear in their shape
  fields, so they carry an extra `∂²x/∂field²·fielḋ²` term and are rejected
  with `DiffError::SecondOrderUnsupported` — a documented, mechanical
  extension.
- **Vertex / Boundary nodes** solve the second-order implicit system
  (`constraint_row_2`). Differentiating the frozen-branch identity
  `∇g·ẋ = −∂g/∂θ` once more,

  ```text
  ∇g · ẍ = −( ∂²g/∂θ²  +  2 ẋᵀ∇ₓ(∂g/∂θ)  +  ẋᵀ ∇²g ẋ ),
  ```

  with the **same** row gradients `∇g` as the velocity solve, so the same
  Gram–Schmidt routine (`solve_vertex_velocity`) recovers `ẍ` with the
  tangential DOF frozen. The rhs splits cleanly: a *field-acceleration* part
  that is the first-order rhs fed the field accelerations
  (`constraint_row(surface, acc_seeds, x).rhs` — the plane's whole share, and
  the `∂²g/∂θ²` term of the quadrics), plus a *velocity-curvature* part
  `−2‖ẋ⊥ − v_c⊥‖² + 2ṙ²` (plane: 0) that uses only the implicit form's constant
  Hessian (`∇²g` = `0` / `2I` / `2P`, `P = I − aaᵀ`). Tangency-completion rows
  are linear in `x` and the surface center, so their second-order form is the
  first-order `tangency_rows` fed the acceleration seeds.

**Implemented for plane, cylinder, sphere** — enough for the boolean-hole and
rounded-cube gates. Cone/torus vertex rows are the mechanical extension
(non-constant `∇²g`).

**What `volume_with_second_derivative` computes, exactly:** `d²V/dθ²` along one
seeded direction, with both the position-curvature term and the
plane/cylinder/sphere node accelerations carried. It is *not* a many-parameter
Hessian (that is Part 1.1's job for least-squares QoIs).

### Gates (`m10_second_order.rs`)

| Model | check | measured |
|---|---|---|
| Boolean hole, `d²V/dr²` | vs closed form `−N·sin(2π/N)·t` (`V` quadratic in `r`) | rel **≤ 1e-9** |
| Boolean hole, `d²V/dr²` | vs central FD of analytic `dV/dr` (`h = 1e-6`) | rel **5.3e-10** |
| Cylinder height, `d²V/dh²` | `V` linear in `h` ⇒ 0 | `|d²V/dh²| < 1e-6` (exact 0) |
| Rounded cube, `d²V/dr²` | vs central FD of analytic `dV/dr` (`h = 1e-3`) | rel **1.2e-3** |

Node-acceleration machinery is exercised both ways: on the rounded cube every
node position is linear in `r`, so the second-order vertex/lift solve correctly
computes **zero** acceleration (asserted: `max‖ẍ‖ < 1e-9`) and `d²V/dr²` is the
position-curvature term alone; the unit test
`implicit::tests::cylinder_radius_acceleration_matches_nonlinear_field` drives
a genuinely nonlinear `r(θ)` and checks the resulting nonzero `ẍ = r̈·û`
against the closed form.

Two honest notes:

- The rounded-cube FD uses `h = 1e-3`, not `1e-6`: a filleted solid re-captured
  at `r ± h` carries O(1e-4) mesh-correspondence jitter (the M9 fillet-frame
  caveat), so a 1e-6 step sits in the noise. At 1e-3 the jitter/2h clears and
  the central difference of the (smooth-in-r) analytic derivative converges to
  the second derivative (rel 1.2e-3; the same value is reached at `h = 1e-2`).
- The boolean-hole gate is exact against the discrete N-gon closed form because
  `V(r)` is genuinely quadratic in `r` with zero node acceleration, so both the
  closed form and `Dual<Dual>` land on `−N·sin(2π/N)·t`.

Cone/torus second-order lift and vertex rows are the only deliberate omission,
documented as mechanical (the linear-field structure that makes plane/cylinder/
sphere a one-liner does not hold there).

---

## Part 2 — performance

The task premise was quadratic scans in capture matching / dedup. **Profiling
found otherwise**, and the milestone rule "only optimize what you can measure"
took over:

- The **cross-face dedup is already spatial-hashed** (`HashMap` over quantized
  cells with a 3×3×3 probe, since the M0–M2 adversarial hardening).
- The **per-face topology-vertex classification** scans a face's boundary loop,
  which is bounded by *topology*, not tessellation density. Grid-accelerating
  it measured **slower** — building a per-face `HashMap` costs more than the
  short linear scan — and worse, near-tolerance classification matches (a mesh
  node ~`VERTEX_MATCH_TOL` from a rim vertex) can straddle to a cell two away,
  so the 3×3×3 probe is not exhaustive there and the plan is not bit-identical.
  Both reasons: **reverted**, left as the linear scan.
- Capture, seam, and pullback are all **linear in node count** (measured
  below).

The one genuinely `O(nodes · vertices)` scan is the **evaluation-time**
(`evaluate_plan`, the finite-difference oracle) resolution of each frozen node
to its nearest rebuilt vertex. Here the accelerator is both safe and a win: a
node's anchor sits ~1e-6 mm from its rebuilt vertex — far inside `MATCH_TOL` —
so the grid's 3×3×3 probe *is* exhaustive and the result is **bit-identical**
to the linear reference (`evaluate_plan_naive`), which the equivalence gate
asserts. A shared `PointGrid` (quantum = the existing `MATCH_TOL`) drives it;
the tie-break is insertion order = `ci.vertices` order, matching the linear
scan's first-on-tie exactly.

### Bit-identical (`m10_perf.rs::m10_grid_evaluation_is_bit_identical_to_linear`)

`evaluate_plan` (grid) vs `evaluate_plan_naive` (linear) produce byte-for-byte
identical meshes — same triangles, node positions equal to the bit — on the
rounded cube at `circle_segments ∈ {16, 64, 128}` and on a flywheel-class
drilled disc (asserted).

### Timings (`m10_perf.rs::m10_capture_seam_pullback_timings`, median ms, debug build)

Capture / seam / pullback on the rounded cube — all linear in node count
(nodes ×7.8 from segs 16→128, capture ×7.5):

| segs | nodes | capture | seam | pullback |
|---|---|---|---|---|
| 16 | 836 | 16.5 | 1.46 | 4.58 |
| 64 | 3284 | 61.6 | 4.08 | 15.9 |
| 128 | 6548 | 123.9 | 7.49 | 30.6 |

`evaluate_plan` naive (`O(nodes·verts)`) vs grid, before/after:

| model | segs | nodes | topo-verts | naive | grid | speedup |
|---|---|---|---|---|---|---|
| rounded cube | 16 | 836 | 24 | 5.98 | 6.36 | 0.94× |
| rounded cube | 64 | 3284 | 24 | 23.3 | 23.7 | 0.98× |
| rounded cube | 128 | 6548 | 24 | 45.8 | 46.1 | 0.99× |
| flywheel | 16 | 728 | 642 | 21.6 | 10.8 | **2.00×** |
| flywheel | 64 | 824 | 642 | 22.0 | 11.3 | **1.94×** |
| flywheel | 128 | 1592 | 642 | 25.0 | 14.3 | **1.75×** |

The win scales with topology-vertex count: the rounded cube carries only 24
(so `O(24·nodes)` is already cheap and the grid's build overhead makes it a
wash — a ~1× no-op), while the flywheel's boolean rims carry 642, where the
grid halves the resolution time. The grid never meaningfully hurts and helps a
lot exactly where the quadratic bites.

## Regression

`cargo test` green on `vcad-kernel-diff`, `vcad-kernel-tessellate`,
`vcad-kernel-geom`, `vcad-kernel-fillet`; `cargo clippy --all-targets -D
warnings` clean on the four; `cargo fmt --all --check` clean.
