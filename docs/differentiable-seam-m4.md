# Differentiable seam — M4 design note

M3 (see `differentiable-seam-m3.md`) closed the loop on primitive
parameters: boundary recipes, mass-property QoIs, and the first geometry
grown by gradient descent. M4 differentiates the parameter the whole
project was aimed at from the start: a **fillet radius** — the canonical
"real CAD" parameter, and the hard case for a frozen-topology seam,
because one θ moves twenty surfaces at once, every moving trim is a
*tangent* contact, and the fillet kernel itself is
construction-order-nondeterministic.

The proving model is the all-edges-filleted cube (the kernel's supported
fillet path): 6 inset planes, 12 quarter-cylinder edge blends, 8
sphere-octant corners. θ = the blend radius `r`. The continuum ground
truth is the Minkowski sum of the shrunken cube with a ball:
`V(r) = a³ + 6a²r + 3πar² + (4/3)πr³` with `a = L − 2r`.

## What landed

### Composite seeds (`ParamSeeding` now composes)

A fillet radius does not perturb any surface through a single field: each
blend cylinder's **radius grows at rate 1 while its axis simultaneously
retreats from the edge** (velocity ±1 per non-axial coordinate), and each
corner sphere does the same in three coordinates. `ParamSeeding` therefore
maps a surface index to a *list* of `SurfaceSeed`s, applied additively by
the lift-bridge and by the implicit rows (`∂g/∂θ` sums over seeds). One
seed per surface — the M0–M3 shape — is just the singleton case, so all
existing call sites kept their meaning.

### Frame transport (`FaceFrame`, `transport_uv`)

The fillet pipeline picks blend-frame axis signs by hash order, so a
rebuild at θ + h can produce the *same geometric surface* with a flipped
axis or rotated `ref_dir` — silently changing what a frozen `(u, v)`
means and poisoning the FD oracle. The plan now snapshots each face's
parameterization frame at capture, and `evaluate_plan` transports frozen
samples through the rebuilt frame instead of trusting raw parameters:
angle/height transport for cylinders and spheres (project the captured
`u = 0` direction into the rebuilt frame), anchor projection for planes.

Face-slot resolution had to harden with it: a slot is matched to the
rebuilt face minimizing the **worst** transported-sample distance over
*all* of the slot's samples, not the first — an edge node shared by two
faces can agree with the wrong face near one sample and diverge
elsewhere, and a first-sample match cached that wrong binding for the
whole slot. Every node is still verified against its capture-time anchor
after transport (`CorrespondenceLost` on failure, never a silent wrong
answer).

### Tangency completion (`tangency_rows`)

M3's closing note predicted the tangent trim would need a second-order
boundary solve. It doesn't — the resolution is cheaper and more
instructive, in two parts:

- **On the tangent line itself** there is nothing to solve. The tangent
  line is `u = const` on the blend, so a frozen-`(u, v)` interior sample
  with the composite seed already tracks it *exactly* (Pillar 2): retreat
  `(0, 1, −1)` plus radial growth `(0, 0, 1)` compose to the true slide
  `(0, 1, 0)` along the support face. The Boundary-upgrade check refuses
  tangent pairs (their Newton system is singular), routing these nodes to
  the lift-bridge — the machinery that was already correct.
- **At the corner vertices** where tangent lines cross, the implicit
  system is genuinely rank-deficient: a curved surface resting
  tangentially on a plane contributes a row parallel to the plane's row,
  so the completion policy ("frozen directions stay put") pinned a vertex
  that actually slides diagonally along the support face — the analytic
  velocity came out 0 where FD said `(±1, ±1, 0)`. The missing
  information is the **tangent-curve constraint**: the vertex must stay
  on the curve where the moving surface touches the plane. `tangency_rows`
  adds it — for a cylinder tangent to a plane with normal `n`, direction
  `q = axis × n` with rhs `q · ẋ_translate`; for a sphere, two such rows
  spanning `n⊥`. First-order rows, no curvature solve, and they vanish
  identically for non-tangent configurations.

Recipe preference during capture also learned curvature rank
(plane < singly-curved < doubly-curved), so a node shared by a blend and
its support plane freezes `(u, v)` on the surface whose parameters pin it
in more directions.

### The gates (`m4_fillet_radius.rs`)

- **Seam vs FD oracle** at `r = 1.5`, `L = 10`: `dV/dr = −72.807`,
  matching central differences at `h = 1e-6` to **1.7e-8** (gate 1e-6) —
  across all 20 simultaneously-moving composite-seeded surfaces, under
  frame-transported correspondence.
- **Node-wise dx/dr** over all 836 mesh nodes: max rel err **3.2e-9**.
- **Exact spot-checks**: interior tangent-line nodes slide at exactly
  `(0, 1, 0)`; the corner vertex at `(r, r, L)` slides at exactly
  `(1, 1, 0)` — the rank-deficient case the tangency rows exist for.
- **Continuum**: 6.7% from the closed-form Minkowski derivative, inside
  the documented polygonization band for 16-segment blends (the exact
  framework agreement is the FD gate; discretization is a property of the
  mesh, not the seam).
- **The loop closes on the fillet itself**: projected gradient descent
  through the seam recovers the radius whose rounded cube hits a target
  volume to 1e-3 (`r* = 2.2` from `r₀ = 1.0`), reading the radius back
  from the built model at every iterate so the seeding stays honest.

## Notes and boundaries

- The model uses `fillet_all_edges`; single-edge `fillet_edges_detailed`
  currently produces a malformed solid (it insets all six faces but emits
  one blend), which is a fillet-kernel bug independent of the seam — the
  seam's topology-signature and anchor checks are exactly the machinery
  that caught it.
- `tangency_rows` covers cylinder-on-plane and sphere-on-plane, the
  contacts fillets and chamfers of planar-faced parts produce. Tangent
  pairs of two curved surfaces (variable-radius blends over curved
  supports) extend the same way: the tangent-curve direction is
  `∇g_a × (∇g_a × ∇g_b)`-degenerate, but the rows stay first-order.
- Frame transport currently covers plane/cylinder/sphere frames — the
  kinds the fillet pipeline emits. Cone/torus transport slots into the
  same `FaceFrame` enum when a consumer needs it.
- Forward mode remains one seam pass per parameter; a part with dozens of
  independent fillet radii is where reverse mode (M5) picks up, behind
  the same `SeamMesh` interface.
