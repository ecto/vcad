# Differentiable seam — M3 design note

M0–M2 (see `differentiable-seam-m0-m2.md`) proved **dx/dθ** correct against
a finite-difference oracle. M3 makes the seam *do* something: the first
geometry improved by gradient descent through it, plus the two pieces of
machinery that unlock it — multi-surface boundary recipes (closing M0–M2's
one known correctness gap) and mass-property QoIs with exact θ-derivatives.

## What landed

### Boundary recipes (`NodeRecipe::Boundary`)

M0–M2's recipe-priority heuristic (`TopoVertex > curved-uv > plane-uv`)
assumed a moving trim always lives on the *curved* surface. The inverse
case — a boundary node shared between a moving plane and a fixed curved
surface, e.g. cylinder **height** as θ — froze the node on the fixed
surface: the frozen mesh differentiated a slightly different body and the
FD oracle, replaying the same recipes, agreed with the wrong answer.

Now a node contributed by two faces on *distinct* surfaces (with
independent gradients) becomes a `Boundary` recipe: its position is the
Newton solution of `{g_a(x) = 0, g_b(x) = 0, t·(x − anchor) = 0}` with `t`
the intersection tangent (the frozen-parameter branch choice), and its
velocity comes from the same two-row implicit system used for rim topology
vertices. The node tracks the moving trim **no matter which surface carries
θ**. Implicit forms `(g, ∇g)` moved to `vcad-kernel-geom::implicit_form`
(their natural home) so both the tessellate-side Newton solve and the
diff-side seeded rows share one source of truth.

`m3_boundary_trim.rs` is the regression test on the previously-wrong case
(cylinder height as θ): rim nodes ride the moving cap at exactly ẑ,
`dV/dh` matches the discrete closed form and FD to ≤1e-6 (measured
~1e-10). The degenerate-cap-with-holes tessellation branch was factored
out of `tessellate_brep` into a shared `tessellate_degenerate_cap` helper
so capture can never drift from the renderer's dispatch (previously it was
a hard `UnsupportedFace` error in capture).

### Mass-property QoIs (`mass_properties`, `mass_properties_with_derivative`)

Volume, mass, centroid, and inertia tensors (about origin and centroid)
via exact signed-tetrahedron integrals, generic over `tang::Scalar` —
`Dual<f64>` node positions give every field's θ-derivative in one pass
(and `Dual<Dual<f64>>` reaches second derivatives when needed). Gates
(`m3_mass_properties.rs`): cuboid closed forms to 1e-12; `dI_zz/dd` of an
extrusion against its closed form and FD; all cylinder-radius derivatives
against FD (≤1e-6, measured ~1e-9).

### The physics hook (`contract_sensitivity`)

`dJ/dθ = Σ_i (∂J/∂x_i)·(dx_i/dθ)` — the contraction a physics functional
plugs into. `volume_gradient` (analytic per-node ∂V/∂x) is the reference
functional; the contraction reproduces the dual-number `dV/dθ` to 1e-12,
which is exactly the consistency a phyz adjoint's `∂J/∂x` will inherit.

**Status of true phyz coupling:** this sandbox cannot build the `phyz`
sibling repo (session repo scope), so the rollout adapter itself — a small
function in `vcad-kernel-physics` mapping a rollout objective's mesh
gradient into `contract_sensitivity` — is deferred to a session with phyz
access. The interface it targets is frozen here and exercised by the
analytic physics objectives (inertia, spin-up ∝ I/τ are mass-property
functions).

### The optimizer harness (`objective_gradient`, `minimize`)

Projected gradient descent with warm-started backtracking. The contract
per iterate: rebuild at θ → **re-capture** a fresh frozen plan (plans are
only valid near their capture point) → one seam evaluation per parameter
(forward mode) → step. Frozen-tessellation errors during a *trial* step
(`TopologyChanged`, `CorrespondenceLost`, `BoundarySolveFailed`) are the
subgradient signals of the seam design and are handled as failed steps
(shrink), never accepted silently. Errors at an accepted iterate
propagate.

### The demo (`m3_flywheel_optimize.rs`)

A disc flywheel with a center bore and four lightening holes;
θ = (bore radius, hole radius); brief: *hit a target spin inertia I_zz
with minimum mass* (`J = m/m_ref + λ((I_zz − I_t)/I_t)²`, λ = 100). From
θ₀ = (3, 2.5) mm, 83 accepted iterations grow the design to
θ = (12.0, 4.96) mm — the bore riding its bound — cutting mass ~13% while
landing I_zz within 1% of target. Monotone descent is asserted iterate by
iterate, and the analytic gradient is audited against the FD oracle at the
start point (≤1e-6; the audit uses h = 1e-4 because J is assembled from
O(1e7) inertia integrals and a 1e-6 step would sit at the oracle's own
roundoff floor). The model exercises a five-boolean chain per iterate —
several hundred boolean builds over the run — with the correspondence
machinery holding throughout.

## Notes and boundaries

- Boundary recipes pair **two** surfaces; a non-topology node on ≥3
  surfaces keeps the first two (such a point is a corner the topology
  should carry; if one ever matters, the recipe generalizes the same way
  the vertex solve already does).
- The optimizer is deliberately the simplest loop that closes; L-BFGS or
  trust regions plug into `objective_gradient` unchanged. Forward mode is
  one seam pass per parameter — right up to a few dozen parameters, after
  which reverse mode (M5) takes over behind the same `SeamMesh` interface.
- Runtime: the flywheel run is ~17 s in a debug build, dominated by ~100
  boolean rebuilds; per-iterate cost is build ≈ 65 ms, capture ≈ 22 ms,
  seam ≈ 1 ms per parameter. The known perf follow-ups (spatial hashing in
  matching, borrowed connectivity) from the M0–M2 note stand.

## What M4 needs from this

Fillet-radius parameters put the moving trim on a *tangent* curve between
a fillet surface and its support faces. The Boundary machinery is the
on-ramp: the Newton system's rows are exactly the fillet's defining
contact equations, but the tangency makes `∇g_a × ∇g_b` degenerate at the
contact line, so M4 needs the curvature-aware (second-order) version of
the boundary solve, plus torus/cone implicit forms in
`vcad-kernel-geom::implicit_form` — both localized, neither touching the
recipe or seam interfaces.
