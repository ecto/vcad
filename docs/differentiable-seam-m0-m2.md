# Differentiable seam — M0→M2 design note

The first executable piece of the differentiable CAD→physics loop: **dx/dθ**,
the sensitivity of tessellated mesh-node positions to a CAD parameter,
computed by cutting the chain at the mesh and never differentiating through
B-rep combinatorics. This note records the resolved ground truth, the design
decisions and their reasons, and the measured finite-difference tolerances
per milestone.

## What landed

- `vcad_kernel_tessellate::frozen` — frozen-tessellation mode: a two-phase
  capture/evaluate split with a topology signature that turns any topology
  change under perturbation into a hard error.
- `vcad-kernel-diff` (new crate) — the seam itself: the lift-bridge
  (`lift_surface` / `DualSurface`), explicit θ→field seeding
  (`SurfaceSeed` / `ParamSeeding`), implicit vertex differentiation
  (`constraint_row` / `solve_vertex_velocity`), assembly
  (`evaluate_with_sensitivity` → `SeamMesh`), a scalar-generic volume QoI,
  and the central-difference oracle (`fd_velocities`,
  `fd_volume_derivative`, `compare_velocities`).
- Three milestone integration tests (`crates/vcad-kernel-diff/tests/`),
  each of which *is* its acceptance gate.

## Step 0 — resolved ground truth

- **Geometry store**: `GeometryStore` in `vcad-kernel-geom`
  (`crates/vcad-kernel-geom/src/lib.rs`), element type
  **`Vec<Box<dyn Surface>>`** (`surfaces` field; `curves_3d` / `curves_2d`
  alongside). It lives on `BRepSolid { topology, geometry, solid_id }`,
  which is defined in **`vcad-kernel-primitives`** (not `-topo` or the
  unified `vcad-kernel`). `Face.surface_index` indexes `geometry.surfaces`.
- The spec's ground truth held up everywhere it was checked: all seven
  surface kinds (`Plane`, `CylinderSurface`, `ConeSurface`, `SphereSurface`,
  `TorusSurface`, `BilinearSurface` in `-geom`; `BSplineSurface` in
  `-nurbs`) carry `lift<T: Scalar>()` plus scalar-generic
  `impl<S: Scalar>` evaluation, `tang::Dual<f64>: Scalar` holds, and the
  `Surface` trait / topology are concrete `f64` and untouched by this work.
  (Minor addition to the map: it's seven kinds, not six — `Bilinear` is also
  generic; and `BSplineSurface`'s generic evaluator is `eval(u: f64, v: f64)`
  with f64 basis functions and generic control-point accumulation, unlike
  the `evaluate(Point2<S>)` shape of the others.)

Corrections to the map that materially shaped the design (all discovered by
running the code, none contradicting the spec's architecture):

1. **`TriangleMesh` stores `f32` positions.** A central-difference oracle at
   `h = 1e-6` is meaningless at `f32` resolution, so the frozen path emits
   its own `f64` `FrozenMesh` and never round-trips through `TriangleMesh`.
2. **The boolean pipeline is order-nondeterministic.** Two builds of the
   same model can enumerate isomorphic topology in different orders
   (hash-map iteration inside the boolean decides e.g. which cap face is
   sewn first; empirically, `cube − cylinder(r)` swaps its two cap faces
   between `r = 2.5` and `r = 2.5 − 1e-6`). Traversal order therefore
   cannot carry node correspondence across rebuilds. Consequences:
   - the topology signature hashes the **sorted multiset** of per-face
     descriptors (surface kind, orientation, loop lengths) plus entity
     counts, so an isomorphic reordering is not flagged as a topology
     change;
   - `evaluate_plan` recovers entity correspondence **geometrically**: each
     node stores its capture-time position, topology vertices resolve to
     the nearest rebuilt vertex, and face slots resolve to the face whose
     surface evaluated at the frozen `(u, v)` lands nearest the stored
     position. Matches farther than 1e-3 mm are a hard
     `CorrespondenceLost` error. This bounds how far a plan may be
     evaluated from its capture point (fine for derivative steps; re-capture
     for large parameter moves).
3. **Boolean sewing does not twin the rim.** After `cube − cylinder`, rim
   vertices carry half-edges of the cap face only (`twin: None`); the hole
   wall keeps a two-half-edge seam loop. Loop membership alone therefore
   under-constrains a rim vertex, so the implicit system collects
   constraint rows by **geometric incidence** (normalized residual
   `|g|/|∇g| < 1e-6 mm` against every surface with an implicit form) in
   union with topological adjacency.
4. **Primitive cylinder caps are degenerate single-vertex circle loops**,
   special-cased in `tessellate_brep` but not in the per-face path; the
   capture mirrors that dispatch (`tessellate_disk_general`).

## The frozen plan

`capture_plan` runs the stock tessellator once at the base θ and classifies
every emitted node:

- **`TopoVertex`** — coincides with a topology vertex (within the stock weld
  quantum). Position tracks the kernel's own output across rebuilds; the
  analytic velocity comes from implicit differentiation (Pillar 3).
- **`SurfaceUv`** — everything else, inverted to frozen `(u, v)` on its
  face's surface (implemented for Plane/Cylinder/Sphere; other kinds error
  honestly). Position is `S(u, v; θ)`; the analytic velocity is the dual
  part of the lift-bridge evaluation (Pillar 2).

Coincident nodes from adjacent faces are merged with the stock weld quantum,
with recipe priority **TopoVertex > SurfaceUv-on-curved > SurfaceUv-on-plane**.
The priority is load-bearing: a cap-rim node captured on both the cap plane
and the cylinder wall must bind to the *moving* cylinder so a fixed-`(u,v)`
evaluation keeps it on the moving trim (a plane binding would freeze it and
open an O(h)-wide crack whose volume error is O(1) in the derivative).

Known limitation, deliberate: the stock hole-aware planar triangulation can
insert Steiner points on outer boundary *edges* (T-junctions against the
neighbouring face's coarser edge). These are geometrically watertight
(zero-area gaps, exact for the volume integral) but show up as odd-parity
edges in `FrozenMesh::open_edge_count`; M2's caps have 12 of them, all on
θ-independent block edges. The milestone gates don't depend on
`open_edge_count == 0` for boolean outputs.

## θ → field seeding

`ParamSeeding` maps **surface indices → `SurfaceSeed`** (`Translate`,
`CylinderRadius`, `SphereRadius`); unlisted surfaces are θ-independent.
Seeds that don't apply to a surface kind are a hard `UnsupportedSeed` error,
never silently ignored, and `seed_where` returns the match count so callers
can assert exactly how many stored surfaces a parameter touches (boolean
outputs can carry several copies of a moving surface). In the milestone
models: M0's extrude distance touches exactly the top cap plane
(`Translate{ẑ}`); M1/M2's radius touches exactly one `CylinderSurface`
(`CylinderRadius{1}`).

## Implicit vertex differentiation (Pillar 3)

Each incident surface contributes a row `∇g · ẋ = −∂g/∂θ` (implicit forms:
plane `n·(x−o)`, cylinder `|radial|²−r²`, sphere `|x−c|²−r²`). The solver is
Gram–Schmidt with the rhs carried: independent rows determine components of
`ẋ`; dependent rows must agree within tolerance or the solve fails with
`InconsistentConstraints`; unconstrained directions are frozen at zero —
the minimum-norm solution, which is exactly the frozen-branch convention
(a rim node keeps its angular parameter). Three planes at a box corner give
the full 3×3 solve; plane ∩ cylinder at a rim gives the rank-2 +
frozen-tangent case.

## Measured tolerances (gate: max relative error ≤ 1e-6)

| Milestone | Check | Measured |
|---|---|---|
| M0 (extrude d, V) | seam dV/dd vs analytic A = 12 mm² | exact (0.0) |
| M0 | seam dV/dd vs FD | 8.3e-12 |
| M0 | node-wise dx/dd vs FD | 2.9e-11 |
| M1 (cylinder r) | node-wise dx/dr vs FD (interior lateral samples exact-radial to 1e-12) | 5.6e-10 |
| M2 (through-hole r, V) | seam dV/dr vs discrete closed form −N·sin(2π/N)·r·t | 1.1e-15 |
| M2 | seam dV/dr vs FD | 6.2e-10 |
| M2 | node-wise dx/dr vs FD (rim nodes exact-radial to 1e-9) | 5.6e-10 |

FD convention: central difference, `h = 1e-6`, rebuilt at θ±h and
re-evaluated under the same frozen plan. Node-wise relative error divides by
`max(|analytic|, |fd|, 0.01 · max-velocity)` so FD roundoff noise
(≈ ε·|x|/2h ≈ 1e-9 mm per unit θ at these model scales) on genuinely
stationary nodes is not amplified into spurious relative error; the floor is
three orders above that noise and three below the gate.

One deviation from a literal reading of the spec: M2's *continuum* closed
form `dV/dr = −2πrt` is not met at 1e-6 by design — the frozen mesh's rim is
the N-gon the boolean produced, so the seam differentiates the discrete
model exactly (1.1e-15 against the N-gon closed form) and differs from the
continuum by the polygonization factor `sin(x)/x`, `x = 2π/N` (6.4e-3 at
N = 32). The M2 test gates the discrete form and the FD oracle at 1e-6 and
asserts the continuum gap equals its discretization bound `x²/6` within 10%.
Discretization error is a property of the mesh, not the seam; it vanishes
as N → ∞ and the seam's derivative is exact for the object the physics side
will actually consume (the mesh).

The topology-signature acceptance is exercised both synthetically
(cube plan vs cylinder) and on a real subgradient crossing (blind hole
perturbed into a through hole): both the frozen evaluation and the seam
return `TopologyChanged`, never a mesh.

## Known limitations (documented deliberately, in scope-fence order)

An adversarial review pass hardened several correspondence paths (per-node
anchor verification everywhere a recipe resolves, ambiguity detection in
nearest-vertex matching, neighbor-cell probing in capture dedup so a
quantization-cell straddle cannot silently split a seam node, reachable-only
edge counting in the signature, dependent-row treatment of nearly-parallel
constraint gradients so their rhs noise is not amplified, and a hard error
for the degenerate-cap-with-holes face the per-face tessellator cannot
mesh). What remains is known and deliberate:

- **Recipe priority assumes the moving trim lives on the curved surface.**
  *(Resolved in M3: coincident nodes on two distinct surfaces are now
  `NodeRecipe::Boundary` — a Newton-tracked intersection point — so the
  node follows the moving trim regardless of which surface carries θ. See
  `differentiable-seam-m3.md`.)*
  `TopoVertex > SurfaceUv-on-curved > SurfaceUv-on-plane` is correct for
  M0–M2's parameters, but a boundary node shared between a *moving plane*
  and a *fixed* curved surface (e.g. cylinder **height** as θ, where the
  rim is not a topology vertex ring) freezes on the fixed surface: the
  frozen mesh then differentiates a slightly different body, and the FD
  oracle — which replays the same recipes — agrees with the analytic side
  while both differ from the true CAD derivative. The general fix is a
  multi-surface boundary recipe (the `SurfaceUv` analogue of the implicit
  multi-row vertex solve); until then θ must move curved surfaces or
  topology-vertex-carried trims. Flagged loudly on `recipe_rank`.
- **Geometric incidence uses unbounded implicit forms.** A vertex exactly
  coincident with the *infinite extension* of an unrelated seeded surface
  (coplanar step heights, coaxial equal-radius bores) picks up a spurious
  constraint row: contradictions surface as a hard
  `InconsistentConstraints` error (loud), but a compatible spurious row on
  an otherwise under-determined vertex is silent. Bounded incidence needs
  trim-region containment — the same machinery as the multi-surface recipe.
- **Tolerances are absolute (mm) module constants.** `1e-3` matching/dedup
  and `1e-4` incidence are correct for mm-scale parts; at ~10 m extents the
  stock tessellator's `f32` rounding approaches the match tolerance, and a
  legitimate parameter step larger than `MATCH_TOL` trips
  `CorrespondenceLost` (re-capture instead). Making them plan-carried
  options is mechanical when needed.
- **The signature is a face-descriptor multiset.** A connectivity rewire
  that preserves the multiset hashes equal by construction; the per-node
  anchor checks are the backstop that turns such a rebuild into
  `CorrespondenceLost` rather than silent garbage.
- **`invert_uv` supports Plane/Cylinder/Sphere.** Cone/torus projection
  exists in `vcad-kernel-booleans::trim` but that crate sits above
  tessellate in the dependency graph; sharing it means moving it into
  `-geom` — a follow-up refactor, out of the minimal-diff budget here.

## Sibling-repo caveat (local validation)

This sandbox could not clone `ecto/loon` / `ecto/phyz` (session repo scope),
which `vcad-loon`, `vcad-kernel-physics`, and `vcad-sim` need as sibling
checkouts. Local validation ran `cargo test`/`cargo clippy -- -D warnings`
on the full workspace **excluding** those crates and their dependents
(`vcad-cli`, `vcad-eval`, `vcad-ffi`, `vcad-kernel-wasm`, `vcad-render`,
`mecheval-grader`; `vcad-app`/`vcad-desktop`/`vcad-chat` are outside the
default build set) — none of which are touched by this change. CI clones
the real siblings and runs the full set.

## What M3 needs from this

A `phyz`-coupled QoI plugs in at `SeamMesh`: `positions` (x, `f64`),
`velocities` (dx/dθ), and frozen `triangles` with stable node identity
across the optimization step. The shape derivative is
`dJ/dθ = Σ_i (∂J/∂x_i) · (dx_i/dθ)` — the physics side supplies `∂J/∂x_i`
(e.g. from a phyz rollout's adjoint or its existing gradients) and contracts
it against `SeamMesh::velocities`; `volume_with_derivative` is the template
(it does exactly this contraction through `Dual` arithmetic, and
`mesh_volume` being scalar-generic means second derivatives via
`Dual<Dual<f64>>` are already reachable). The loop per optimizer step:
build(θ) → `capture_plan` → `evaluate_with_sensitivity(brep, plan, seeding)`
→ physics QoI on (positions, velocities) → step θ → **re-capture** (plans
are valid only near their capture point; `TopologyChanged` /
`CorrespondenceLost` are the signals that a step crossed a subgradient and
must be handled by the optimizer, e.g. by shrinking the step). Multi-θ is a
loop over seedings for now (forward mode, one pass per parameter); reverse
mode over many parameters is M5's business and slots in behind the same
`SeamMesh` interface.
