# Differentiable seam — M9 design note

M5 (`differentiable-seam-m5.md`) built the adjoint seam: one
`evaluate_with_pullback` transposes a mesh functional's gradient into
per-surface cotangents, and `MeshCotangents::contract` prices any parameter
with a handful of dot products. M5 left the optimizer wired to forward mode —
`objective_gradient` / `minimize` still paid one seam pass per parameter.
M9 closes that gap: a reverse-mode gradient path for the optimizer, an
L-BFGS driver over it, and a five-parameter model that only pays for itself
once per iterate.

## What landed

### Reverse-mode objective gradients (`MeshObjective`, `objective_gradient_reverse`)

A mesh-space objective reports its value and its per-node position gradient:

```rust
pub trait MeshObjective {
    fn value_and_mesh_gradient(&self, seam: &SeamMesh) -> (f64, Vec<Vec3>);
}
```

`objective_gradient_reverse(build, seeding_for, objective, θ, params)` then
costs, per iterate: one rebuild, one capture, **one** positions-only seam
pass (empty seeding — the objective only needs where the nodes are), **one**
`evaluate_with_pullback`, and `n` `contract` calls. That is against the
existing `objective_gradient`'s `n` full seam passes. The two share `build`
and `seeding_for` verbatim, so a problem posed for forward mode ports to
reverse mode by swapping the objective for its `MeshObjective` form and
nothing else.

`VolumeMatch { target }` is the reference impl: `J = ((V − target)/target)²`
with the analytic mesh gradient `(2·miss/target)·∂V/∂x` built on
`volume_gradient`. Every other mesh QoI follows the same shape — the M9 gate
objective (below) is a five-QoI combination whose per-node gradients are the
same divergence-theorem pattern extended from `volume_gradient` to the first
and second polynomial moments.

The forward `objective_gradient` is untouched. Consistency is a test, not a
`debug_assert`, because it needs a forward objective counterpart to compare
against: gate 1 of `m9_many_parameters` checks reverse == forward per
component at ≤1e-11 relative.

### L-BFGS with box projection (`minimize_lbfgs`)

A two-loop-recursion L-BFGS (memory 8) over `objective_gradient_reverse`,
reusing `OptimizeOptions` / `StopReason` / `IterateRecord` / `OptimizeResult`
unchanged. It keeps the GD loop's discipline — box projection, and
frozen-tessellation errors during a trial step (topology change / lost
correspondence / failed boundary solve) treated as failed steps and shrunk,
never silently accepted — and adds a backtracking Armijo line search whose
sufficient-decrease test measures the *projected* displacement `trial − θ`,
so a step flattened against a bound is judged honestly. The natural
quasi-Newton trial length is 1 once curvature exists; only the first
(steepest-descent) line search uses `initial_step`.

#### L-BFGS near the seam's subgradients — the policy

Quasi-Newton curvature memory assumes a smooth objective; the frozen seam is
smooth only inside a topology class, with subgradient walls at the edges.
Three rules keep a corrupted curvature pair out of the memory:

1. **Positive curvature only.** A pair `(s, y)` is stored only when
   `s·y > εₖ‖s‖‖y‖` (`εₖ = 1e-12`); a non-positive inner product carries no
   usable convexity and would make the inverse-Hessian estimate indefinite,
   so it is dropped.
2. **Subgradient-straddling pairs are dropped.** If the line search that
   produced an accepted step had to shrink *past a frozen error* to get
   there, the accepted `(s, y)` straddles a topology wall and its `y` mixes
   two smooth branches. The step is still taken; the curvature it implies is
   discarded.
3. **Restart on failure.** If the two-loop direction is not a descent
   direction, or the line search finds no accepted step, the memory is
   cleared and the next iterate falls back to projected steepest descent —
   the always-correct direction — before rebuilding curvature. Only when the
   memory is *already* empty does a failed line search terminate the run
   (`StepConverged`).

### The many-parameter gate (`tests/m9_many_parameters.rs`)

The model is a rounded, drilled box —
`fillet_all_edges(cube(sx, sy, sz), r)` with a centered ẑ through-hole of
radius `r_hole` — with **five genuinely independent parameters**
`θ = [sx, sy, sz, r, r_hole]`. The build stays clean through the boolean:
6 planes, 12 fillet-blend cylinders, 8 corner spheres, 1 hole-wall cylinder,
916 mesh nodes at 16-segment blends. Seedings are hand-written (seeding
synthesis is a later milestone): each dimension translates its far face, its
adjacent edge blends and corner spheres (pinned at `dim − r`), and the
recentring hole (at half rate, since the hole sits at `(sx/2, sy/2)`); the
fillet radius drives all twenty blends with composite radius-plus-retreat
seeds (the M4/M5 recipe generalized per-axis); the hole radius drives the one
hole-wall cylinder.

The recovery objective is the squared relative miss of five QoIs — volume,
the three centroid components, and `I_zz` about the origin (`= P_xx + P_yy`
at ρ = 1) — enough to pin all five parameters: the centroid components fix
the dimensions, volume and inertia separate the two radii (the fillet removes
material at large moment arm, the hole at small).

Measured (debug build):

| Gate | Result |
|---|---|
| Reverse == forward `objective_gradient`, per component | ≤ **1.4e-16** relative (gate 1e-11) |
| FD spot-check at h = 1e-6 | `r_hole` **2.4e-10**, `sy` **8.0e-7** ≤ 1e-6 (gate: ≥ 2 components) |
| Local identifiability: 5×5 QoI Jacobian | **det 3.5e4**, min pivot **0.33** — well away from singular |
| Recovery from a distinct θ₀ | L-BFGS `GradientConverged`, **30 iters**, `‖θ − θ*‖∞ = 4.7e-8` (gate 1e-3) |

θ* = `[10, 12, 8, 1.8, 2.2]`, θ₀ = `[8, 14, 6, 1.2, 1.6]`.

The FD spot-check is a spot-check, not a per-component gate: rebuilding a
filleted, drilled solid under a plan captured elsewhere carries the M4
fillet-frame / correspondence noise, which dominates the FD estimate for the
dimensions and the fillet radius (≈1e-4) while leaving the hole radius and
one in-plane dimension clean. Gate 1 — reverse against the trusted
dual-number mass-property pipeline, two independent implementations agreeing
to machine precision — is the exact correctness check.

## Cost: forward n seam passes vs reverse 1 pullback + n dots

Per-iterate **differentiation** cost on the gate model (5 parameters,
916 nodes, 20 reps, debug build), isolated from the shared build + capture:

| Phase | Time / iterate |
|---|---|
| Shared build + capture | 50.6 ms |
| Forward differentiation (5 seam passes) | 21.5 ms |
| Reverse differentiation (1 seam + 1 pullback + 5 dots) | 7.8 ms |

**Reverse mode is 2.8× cheaper** at the differentiation step at 5 parameters,
and the gap widens linearly: forward pays one seam pass per added parameter,
reverse pays one dot product. (Build + capture dominate the wall clock at this
segment count and are identical for both modes, so the full-call ratio is
smaller — the differentiation ratio is the one that scales with parameter
count.)

The compounding win is in the optimizer, not just per iterate. On the gate
problem L-BFGS reached the optimum in **35 objective evaluations**; projected
GD (`minimize`) had not converged after its 200-iteration budget
(**394 evaluations**, `‖θ − θ*‖∞ ≈ 6.5e-2`) — steepest descent zig-zags on
the anisotropic dimension-vs-radius objective that L-BFGS's learned scaling
handles directly. Combined with the 2.8× cheaper gradient, that is roughly a
**30× reduction in seam evaluations** to solve this five-parameter problem.

## Notes and boundaries

- The reverse path presumes seedings the forward path would accept
  (`MeshCotangents::contract` is a plain bilinear form and cannot validate
  them); the M9 seedings are the same ones the forward gate feeds
  `objective_gradient`, and gate 1's agreement is exactly that guarantee.
- `minimize_lbfgs` takes a `MeshObjective`; `minimize` still takes the
  forward `Fn(&SeamMesh) -> (f64, dJ/dθ_k)`. The two objective forms are
  written from the same QoI math in the gate so the comparison is apples to
  apples.
- The subgradient-straddling drop (rule 2) is conservative: it discards a
  pair whenever the line search touched a frozen error, even if the final
  accepted step is well inside the topology class. On the gate model the box
  bounds keep every iterate in one class, so this path is rarely taken; it
  exists so that a run which does graze a wall degrades to steepest descent
  rather than trusting a poisoned Hessian estimate.
- The 5×5 QoI-Jacobian identifiability check is local (finite-differenced at
  θ*); global uniqueness of the discrete minimizer is not claimed, but the
  targets are computed at θ* under the same discretization, so `J(θ*) = 0`
  and the recovery to `‖θ − θ*‖∞ = 4.7e-8` is the operative proof that the
  five parameters are recoverable.
