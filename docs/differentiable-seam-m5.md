# Differentiable seam — M5 design note

M0–M4 (`differentiable-seam-m0-m2.md`, `-m3.md`, `-m4.md`) built forward
mode: pick a parameter θ, seed the surfaces it moves, run one seam pass,
read dx/dθ everywhere. That is the right shape up to a few dozen
parameters — and exactly the wrong shape beyond, because each θ pays a
full pass. M5 adds **reverse mode**: one adjoint pass prices *every*
parameter of a design at once.

## The transpose, not an AD framework

The insight that keeps this small: node velocities are **linear in the
seed values**. A seam evaluation is one linear map per node from
"per-surface seed slots" — each surface's translation velocity (ℝ³) and
radius rate (ℝ) — to that node's velocity. Reverse mode is just the
transpose of those maps, contracted with a mesh functional's gradient:

```text
∂J/∂(seed slots of surface s)  =  Σ_i  A_{i,s}ᵀ (∂J/∂x_i)
dJ/dθ_k                        =  Σ_s ⟨cotangent_s, seeds_k(s)⟩
```

`evaluate_with_pullback(brep, plan, ∂J/∂x)` computes the per-surface
cotangents once; `MeshCotangents::contract(seeding_k)` is then a handful
of dot products per parameter — no further seam evaluations, no tapes, no
graph capture. `n` parameters cost one pullback + `n` dot products
instead of `n` forward passes.

## What landed

### `row_pullbacks` — the transposed vertex solve

The implicit vertex solve (Gram–Schmidt with the rhs carried) is linear
in the rhs vector, with coefficients fixed by the row gradients alone.
`row_pullbacks` runs the same elimination carrying each basis coefficient
as a linear functional over the rows instead of a scalar, and returns
`m_j = ∂ẋ/∂rhs_j`. Rows dropped as dependent get a zero column — their
rhs never enters the solution (in forward mode it is only
consistency-checked). This matters more than it looks:

**Why not just probe with basis seedings through the forward solve?**
Because a moving surface can have duplicate copies in a boolean's store
(the case `seed_where` exists for). Seeding one copy at a time through
the joint solve makes its row contradict the unseeded twin's — the
forward consistency check would (correctly!) reject every probe. The
transpose never needs a joint solve per basis: basis probing happens
strictly per row, through the same row constructors the forward pass
uses.

### Per-row seed-Jacobians by forward probing

No formula is re-derived. Each row's rhs-dependence on its owning
surface's seeds is read off by re-materializing that row with unit basis
seeds (three unit translations; a unit radius rate where the kind has
one) through `constraint_row` / `tangency_rows` themselves — `RowSource`
records which constructor and which output row. Lift-bridge nodes do the
same through `lift_surface` with basis seeds (cached per face slot).
Forward and reverse cannot disagree about a row's seed dependence,
because they ask the same code.

### Shared skeleton

The forward pass's per-node machinery was factored (`checked_index`,
`incidence_context`, `vertex_incident_surfaces`, `assemble_vertex_rows`)
and both modes now walk it: same signature enforcement, same anchor
checks, same incidence union, same tangency completion. The refactor is
behavior-preserving for forward mode — the M0–M4 suites run unchanged.

### The contract

`contract` is a plain bilinear form; it cannot *detect* an invalid
seeding (wrong seed kind for a surface, or duplicate copies seeded
unequally). Detection stays where it always lived — the forward solve —
and the pullback documents that it presumes seedings the forward path
would accept. This is the same division of labor as
`fillet_edges_detailed` vs the signature checks: validation happens on
the primal path, and the derivative machinery refuses to silently guess.

## Gates (`m5_reverse_mode.rs`)

Reverse-vs-forward agreement (same rows, different linear-algebra order),
measured:

| Model class | dV/dθ forward | reverse rel err |
|---|---|---|
| Cube, moving top face (topology vertices) | 12 (exact) | 0 |
| Boolean through-hole, radius (rim vertices + lift bridge) | −78.036 | 5.5e-16 |
| Cylinder height (Boundary trim rings) | 77.646 | 1.8e-16 |
| Rounded cube, dV/da (composite + tangency rows) | 293.665 | 2.7e-15 |
| Rounded cube, dV/dr | −72.807 | 5.9e-16 |

The flagship test prices **both** parameters of the all-edges-filleted
cube — edge length `a` and fillet radius `r`, 20 moving surfaces with
composite seeds — from **one** pullback, then checks each against forward
mode (≤ 3e-15), against the FD oracle (1.7e-9 and 5.6e-9 vs the 1e-6
gate), and dV/da against the Minkowski closed form
`3s² + 12sr + 3πr²`, `s = a − 2r` (0.18% gap at 16-segment blends).

## Notes and boundaries

- The cotangent space is the current seed vocabulary: translation ℝ³ +
  radius ℝ per surface. New `SurfaceSeed` kinds (cone half-angle, torus
  radii) extend `SurfaceCotangent` with one slot and `radius_basis` with
  one arm; nothing else changes.
- The pullback costs ~4 row re-materializations per vertex row and ~4
  lift evaluations per interior node — a small constant over one forward
  pass. The crossover vs forward mode is therefore at *n ≈ small
  constant*: reverse wins from a handful of parameters up.
- `objective_gradient` / `minimize` still drive forward mode (they need
  per-node velocities for their FD audit machinery). Wiring the optimizer
  to prefer pullback when the objective exposes a mesh gradient is
  mechanical and deferred until a many-parameter optimization demands it.
- The phyz coupling this was built for — a rollout adjoint supplies
  `∂J/∂x`, the pullback prices every CAD parameter of the robot at once —
  still needs the `phyz` sibling repo in the session scope; the interface
  (`∂J/∂x` in, cotangents out) is frozen here and exercised by the
  analytic volume functional.
