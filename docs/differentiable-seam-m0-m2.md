# Differentiable seam — M0→M2 (lift-bridge + frozen tessellation)

This note records the first executable slice of the differentiable CAD→physics
loop: the machinery for computing **dx/dθ**, the sensitivity of tessellated
mesh-node positions `x` to a CAD parameter `θ`, "the cheap correct way."

The design decision that bounds the work is **cutting the chain at the mesh, not
inside the B-rep combinatorics**: every geometric predicate (orientation,
in/out, branch selection) is evaluated on the primal `θ` only; the derivative is
then propagated along the *frozen* branch. We never differentiate through a
comparison.

## 0. Resolved ground truth (step 0)

Where the task's "Ground truth" map was verified against the live code, and
where it drifted:

| Claim | Reality |
|---|---|
| Geometry store element type | **`Vec<Box<dyn Surface>>`.** `GeometryStore { surfaces: Vec<Box<dyn Surface>>, curves_3d, curves_2d }` is defined in `crates/vcad-kernel-geom/src/lib.rs` and held on `BRepSolid.geometry` (in `crates/vcad-kernel-primitives`). `Face.surface_index: usize` indexes `surfaces`. |
| Cylinder struct name | It is **`CylinderSurface`**, not `Cylinder`. Field `radius: S`. |
| `Surface` trait | Concrete-`f64` and object-safe as claimed, but has more methods than listed: `evaluate`, `normal`, `d_du`, `d_dv`, `domain`, `surface_type`, `clone_box`, **`as_any`** (the downcast hook the lift-bridge needs), `transform`, `offset`. |
| Generic surfaces + `lift` | Confirmed. Every surface struct has `impl<S: Scalar>` with generic `evaluate`/`normal`/`d_du`/`d_dv`, plus a concrete `lift::<T: Scalar>() -> Self<T>`. Verified for `Plane`, `CylinderSurface`, `ConeSurface`, `SphereSurface`, `TorusSurface`, `BilinearSurface`. |
| `Dual<f64>: Scalar` | Confirmed (`tang/crates/tang/src/dual.rs`): `Dual::new(real, dual)`, `Dual::constant`, `Dual::var`, fields `.real`/`.dual`. Forward-mode AD flows through any generic `evaluate`. |
| `vcad_kernel_math::Point3` etc. | Are **type aliases for `tang::Point3<f64>`** (and `Vec3`, `Point2`, `Dir3`). So f64 store surfaces and tang generic types are the *same* types — no conversion layer. |
| Sibling repos | Besides `../tang`, the workspace also path-depends on **`../loon`** and **`../phyz`**; all three must be cloned next to `vcad` or `cargo` cannot resolve the workspace. |

Nothing here contradicted the core invariant: **topology is index-based**
(`Face.surface_index`), so freezing topology while swapping a typed surface
array is native to the data model, not something we impose.

## 1. Where the code lives (minimal, additive)

- **Lift-bridge** — `crates/vcad-kernel-geom/src/diff.rs`
  (`SurfaceSeed`, `eval_surface_dual`, `SeedMismatch`). Placed next to the
  surfaces and their `lift` methods.
- **Frozen tessellation + oracle + Pillar 3** —
  `crates/vcad-kernel-tessellate/src/frozen.rs` and `frozen/models.rs`
  (`FrozenTessellation`, `TopoSignature`, `ParametricModel`, `audit`,
  `signed_volume`, `implicit_sensitivity`, and the `ExtrudedBox` /
  `BlockWithHole` models). `tang` was added as a direct dependency of
  `vcad-kernel-tessellate`.
- No existing code was refactored; the `Surface` trait and the geometry store
  stay `f64` and non-generic.

## 2. The lift-bridge (Pillar 2)

`eval_surface_dual(surface: &dyn Surface, seed: &SurfaceSeed, u, v)
-> Result<Point3<Dual<f64>>, SeedMismatch>`:

1. downcast the `dyn Surface` back to its concrete struct via `as_any()`;
2. `lift::<Dual<f64>>()` (all fields become constant duals);
3. **seed the θ-dependent field's dual part**;
4. evaluate the generic surface at the **frozen** `(u,v)` (constant duals).

The returned point's `.real` is the position; its `.dual` is `dx/dθ`.

### θ→field seeding scheme

`SurfaceSeed` is the explicit, testable θ→field map:

| Seed | θ-dependent field | Seeded dual | `dx/dθ` |
|---|---|---|---|
| `Frozen` | none (any surface kind) | — | `0` exactly |
| `PlaneTranslate { rate }` | `Plane::origin` | `origin.dual = rate` | `rate` |
| `CylinderRadius` | `CylinderSurface::radius` | `radius.dual = 1` | outward radial dir |

`Frozen` needs no downcast (it uses the `f64` trait `evaluate` and lifts to a
constant dual), so it works for **all seven** surface kinds; a surface a given
`θ` does not touch is `Frozen` and its sensitivity is identically zero. Applying
a seed to the wrong kind returns `SeedMismatch` rather than silently
misbehaving. New parameters extend by adding a seed arm (a few lines each,
because `lift` + generic `evaluate` already exist).

## 3. Frozen-tessellation mode (the genuinely new code)

`dx/dθ` is only meaningful if node `i` at `θ` is node `i` at `θ±h`. A
`FrozenTessellation` models a mesh as a **θ-independent** structure:

- `nodes: Vec<SampleAddr>` — frozen `(surface_index, u, v)` parametric addresses;
- `tris: Vec<[u32;3]>` — fixed connectivity;
- `seeds: Vec<SurfaceSeed>` — per surface, how it moves with `θ`.

Only *surface field values* change with `θ`; the sample pattern, vertex
ordering, and connectivity never do. `positions()` gives primal `f64` nodes;
`positions_dual()` gives position + `dx/dθ` through the lift-bridge; both
preserve node order exactly, so the finite-difference oracle in `audit()` can
difference node-by-node.

### Topology signature (the correctness line)

`TopoSignature` = `{ n_vertices, n_triangles, n_edges, connectivity_hash,
orientation_hash }`. `connectivity_hash` is structural (FNV-1a over the sorted
unique edge set) and thus θ-independent. `orientation_hash` folds the **sign of
each triangle's signed-tet contribution** and is θ-**sensitive** with a `1e-9`
dead-band: a perturbation that inverts or degenerates a triangle flips it.

`audit()` computes the signature at `θ`, `θ+h`, `θ−h` and returns
`AuditError::TopologyChanged` — never a number — if it is not invariant. This is
tested both ways: a valid step keeps the signature; a hole whose radius sits one
`h` inside the block half-width (so `θ+h` pushes the hole *through the wall*)
trips the guard and errors.

### Volume oracle precision

Volume uses signed tetrahedra (`V = ⅙ Σ vᵢ·(vⱼ×vₖ)`), generic over the scalar
so the same code runs on `f64` and `Dual<f64>`. The FD of volume is computed as
`Σ (tet(θ+h) − tet(θ−h))` (difference *before* summing) so frozen, r-independent
triangles cancel at full precision; the naive `V(θ+h) − V(θ−h)` on a large mesh
is dominated by f64 cancellation (~5·10⁻⁷ rel for M2) rather than by the
derivative. The stable form recovers ~5·10⁻¹⁰.

## 4. Pillar 3 (first workout in M2)

A moving trim/intersection point satisfies a defining system `F(x, θ) = 0`;
then `dx/dθ = −F_x⁻¹ F_θ`. `implicit_sensitivity(sys, x, θ)` forms **both**
Jacobians by forward-mode dual seeding (reusing `tang`, no hand-derived
derivatives) and solves the 3×3 system with `Mat3::try_inverse`. For the M2
through-hole rim `{z = t} ∩ {x²+y² = r²}` pinned to angle `φ`, this reproduces
the closed-form radial `(cosφ, sinφ, 0)` and agrees with the lift-bridge value
at the same node.

## 5. Milestones & achieved FD tolerances

Central-difference oracle, `h = 1e-6`, gate = **max relative error ≤ 1e-6**.

| Milestone | Check | Achieved |
|---|---|---|
| **M0** extrude volume | node `dx/dd` vs FD | `1.4e-10` |
| | `dV/dd` analytic vs FD (and vs closed form `sx·sy`) | `2.4e-10` |
| **M1** plane interior sample | node `dx/dθ` vs FD | `1.4e-10` |
| **M1** cylinder interior sample | node `dx/dr` vs FD | `4.7e-10` |
| **M1** topology guard | topology-changing step | **errors** (as required) |
| **M2** through-hole total | `dV/dr` analytic vs FD | `4.8e-10` |
| | `dV/dr` vs *discrete* closed form `−t·N·r·sin(2π/N)` | `<1e-9` |
| | `dV/dr` vs *continuous* `−2π r t` (N=4096) | `3.9e-7` |
| **M2** rim (Pillar 3) | implicit `dx/dr` vs FD | `2.8e-10` |
| | implicit vs analytic `(cosφ, sinφ, 0)` | `2.2e-16` |

Note on M2's continuous closed form: the hole is discretized into `N` sectors,
so the mesh's *exact* `dV/dr` is the polygonal `−t·N·r·sin(2π/N)`. The
analytic-vs-FD gate (framework correctness) is discretization-independent and
lands at ~1e-9; the gap to the *continuous* `−2π r t` is the discretization
residual `O((2π/N)²)`, driven under the gate by `N = 4096`
(`3.9e-7 < 1e-6`). This is the one place the "block" is realized as a prism with
a polygonal hole — the outer cross-section is fixed and r-independent, so it
does not affect `dV/dr` at all, and the closed form holds regardless of the
outer boundary shape.

## 6. What M3 needs from this ("start warm")

M3 couples a `phyz`/`tang` functional `J(x)` and needs the shape derivative
`dJ/dθ = (dJ/dx)·(dx/dθ)`. This slice already delivers the right-hand factor in
the shape/order a physics functional expects:

- `FrozenTessellation::positions(&store) -> Vec<Point3>` gives the node array
  `x` in a **fixed order** — the same order a `phyz` mesh/body would be built
  from, so `dJ/dx` (a per-node covector, same ordering) contracts directly.
- `FrozenTessellation::positions_dual(&store) -> Vec<Point3<Dual<f64>>>` gives
  `x` and `dx/dθ` (the `.dual` parts) node-aligned with the above. The full
  `dJ/dθ` is then `Σᵢ dJ/dxᵢ · (dxᵢ/dθ)` — a single dot product over nodes; no
  new differentiation machinery is required on the CAD side.
- For many parameters, `positions_dual` is called once per `θ` (forward mode);
  M5's reverse-mode pass would instead seed `dJ/dx` and pull back — the seam's
  `SurfaceSeed`/`DefiningSystem` split is the natural place to add that adjoint.
- The `audit()` topology guard is the invariant M3 must respect: a physics step
  that moves `θ` across a `TopologyChanged` boundary must re-freeze topology on
  the new primal before trusting any `dx/dθ`.

The concrete interface a physics functional plugs into is therefore just:
`build(θ) -> GeometryStore` + `tessellation() -> FrozenTessellation`
(`ParametricModel`), then `positions_dual` for the node-aligned `(x, dx/dθ)`
pair.
