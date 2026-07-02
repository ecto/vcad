# Differentiable seam — M7 design note

M0–M5 built the seam on planes, cylinders, and spheres — the surface kinds a
box, a bore, and a fillet of a planar-faced part produce. M7 extends **every
seam extension point** to the two curved kinds that fillets and chamfers of
*curved* parts need: the **cone** and the **torus**. Nothing about the seam's
architecture changes; each of the six extension points gains two arms.

## What landed

One arm per new surface kind at each seam extension point, kept mutually
consistent:

1. **Implicit forms** — `vcad_kernel_geom::implicit_form` (the `(g, ∇g)` the
   Newton/boundary path uses) and `vcad_kernel_diff::implicit::implicit_terms`
   (the single diff-side source of truth for both constraint rows and
   incidence residuals) gained cone and torus arms, algebra identical
   term-for-term so both paths can share a vertex.
2. **`SurfaceSeed`** gained `ConeAngle { rate }`, `TorusMajorRadius { rate }`,
   and `TorusMinorRadius { rate }`. The cone struct stores its opening as a
   **half-angle**, not a base radius, so `ConeAngle` seeds that (a base radius
   at fixed apex distance maps in via `rate = d(atan(R/L))/dθ`). `Translate`
   works for both kinds.
3. **Lift-bridge** — `lift.rs` writes `ConeAngle` into `half_angle.dual` and
   the two torus rates into `major_radius.dual` / `minor_radius.dual`, through
   the structs' existing `lift::<Dual<f64>>()`.
4. **Frame transport** — `FaceFrame::Cone` / `Torus`, plus `face_frame` and
   `transport_uv` arms. `invert_uv` (frozen capture) now inverts cone and
   torus samples.
5. **Reverse mode** — `SurfaceCotangent` gained its own slot per new scalar
   (`cone_angle`, `torus_major`, `torus_minor` — the torus's two radii are
   **never** overloaded onto one slot). `radius_basis` was generalized to
   `scalar_bases`, a per-kind list of `(basis seed, slot)`; both the
   lift-bridge and the row pullbacks accumulate uniformly. Plane/cylinder/
   sphere behavior is byte-identical.
6. **Tangency rows** — left untouched (see below).

Plus one enabling fix in the tessellator (see "Torus constructibility").

## The cone implicit form

vcad parameterizes a cone by apex `a`, unit axis `d` (apex → base), and
half-angle `α`:
`P(u, v) = a + v·(cos α·d + sin α·(cos u·ref + sin u·y))`, `y = d × ref`.
A point is on the ruled surface iff its radial extent equals `tan α` times its
axial extent, giving

```text
axial  = (x − a)·d
radial = (x − a) − axial·d
g      = |radial|² − tan²α · axial²
∇g     = 2 radial − 2 tan²α · axial · d
```

(both nappes are zeros of `g`; the primitive only meshes the forward one).
`radial ⊥ d`, so `∇(|radial|²) = 2 radial` with no projection term. The
seeded derivatives:

- `Translate v`: `∂g/∂θ = −∇g·v` (the whole form rides the apex).
- `ConeAngle`: only `tan²α` depends on `α`, so
  `∂g/∂α = −axial²·2 tan α·sec²α`.

## The torus implicit form

Center `c`, unit axis `d`, major `R`, minor `r`:

```text
axial    = (x − c)·d
p_radial = (x − c) − axial·d
ρ        = |p_radial|
g        = (ρ − R)² + axial² − r²
∇g       = 2(ρ − R)/ρ · p_radial + 2 axial · d
```

The `1/ρ` is the one degeneracy: on the axis `p_radial → 0` and the radial
direction (hence `∇g`) is undefined, so both implicit functions return `None`
for `ρ < 1e-12`. A torus *surface* point never hits it (`ρ = R + r cos v > 0`
for `R > r`). Seeded derivatives: `∂g/∂R = −2(ρ − R)`, `∂g/∂r = −2r`, and
`Translate v` is again `−∇g·v`.

## Gauge and observability

The interesting new gauge question is the **cone apex slide along its axis**.
For a plane, in-plane translation is unobservable (frame transport projects it
away); for a cylinder, an axial center slide is unobservable (the surface is
translation-invariant along its axis). A cone is **not** axis-translation-
invariant: sliding the apex along `d` rescales the radius at every fixed
height (`r(z) = (z_apex − z)·tan α` moves with `z_apex`), so apex-along-axis
motion **is** observable and carries a real `∂g/∂θ = −∇g·v ≠ 0`. The same is
true of opening the half-angle at a fixed apex — the radius at any fixed height
grows — which is exactly why the `ConeAngle` gate below is a well-posed volume
derivative even though the apex never moves. The torus has no translation
gauge at all (it is invariant under no translation), and its axis may flip
between builds only together with `(u, v) → (−u, −v)`, which frame transport
absorbs.

Frame transport reflects these intrinsics: a cone's apex and axis are
intrinsic (the axis names *which* nappe is meshed and cannot flip to the same
surface), so `transport_uv` corrects only a rotated `ref_dir`; a torus's
center is intrinsic but its axis may flip, so torus transport reconstructs two
frame-independent directions (the toroidal tube-center direction and the
poloidal/normal direction) and re-reads their angles — absorbing a rotated
`ref_dir` and an axis flip together.

## Tangency rows: untouched, by contract

`tangency_rows` returns no rows for cone and torus kinds — and that is
correct, not a gap. The gates M7 ships (a cone growing/tapering against flat
caps, a bare torus) never produce a rank-deficient tangent vertex where a
curved surface rests tangentially on a plane along a ruling; the cone∩plane
cap rims are **transverse** intersections fully handled by the ordinary
implicit `Boundary` rows. A cone-tangent-to-plane contact (a chamfer blend on
a curved support) would add a tangent-direction row exactly like the cylinder
arm; it is deferred until a gate model produces one, consistent with the
function's documented "unknown kind = no tangency information" contract.

## Torus constructibility (probed)

`make_torus` **does** exist and builds a valid single-face torus B-rep (one
seam vertex, a 4-half-edge loop). But the *tessellator* could not mesh it:
`tessellate_toroidal_face` derives its `(u, v)` extent from the loop's
vertices — written for **trimmed** fillet-blend patches whose corners span a
rectangle — and a full torus's single shared seam vertex collapses that extent
to a point (0 triangles, volume 0). M7 adds a one-line guard: when the derived
span is degenerate, tessellate the whole `2π×2π` donut (trimmed patches are
unaffected). With that, the full torus meshes watertight and **the full solid-
level gate battery applies to the torus**, not just unit tests. (The
`tessellate_brep` render path still routes a full torus to a planar fallback —
a pre-existing rendering gap unrelated to the seam; the capture path uses
`tessellate_face`, which dispatches correctly.)

## Gates (`tests/m7_cone_torus.rs`)

Frustum cone (`make_cone(5, 3, 8)`, apex virtual at z = 20, so no degenerate
apex vertex); bare torus (`make_torus(8, 3)`); N = 24, FD central difference at
h = 1e-6; node-wise floor as in M0. Measured:

| Model | θ | dV/dθ | vs FD (rel) | node-wise dx/dθ | reverse vs forward |
|---|---|---|---|---|---|
| Cone | half-angle α (fixed apex) | 3449.540 | 5.9e-12 | 9.6e-11 | 1.3e-16 |
| Cone | height h (top cap `Translate`) | 27.952 | 4.7e-9 | 1.8e-7 | 6.4e-16 |
| Torus | minor radius r | 926.032 | 4.5e-10 | 1.4e-9 | 1.2e-15 |
| Torus | major radius R | 173.631 | 3.2e-9 | 1.3e-9 | 9.8e-16 |

Discrete closed forms (gated to ≤ 1e-6, where cheap):

- **Cone half-angle**: the mesh is an exact N-gon prismatoid,
  `V = (h·k/3)(Rb² + Rt² + Rb·Rt)`, `k = ½N·sin(2π/N)`, `Rb = 20 tan α`,
  `Rt = 12 tan α`; `dV/dα` matched analytically.
- **Cone height**: `dV/dh` = top cross-sectional area = N-gon area of the top
  radius, `k·Rt²` (the cone analogue of M3's cylinder-height gate, but the rim
  now rides *inward* along the sloped ruling).
- **Torus**: continuum `V = 2π²Rr²`, `dV/dr = 4π²Rr`, `dV/dR = 2π²r²` sit ~2.3%
  above the discrete mesh (two N-gon polygonizations, major and minor) — a
  sanity band; the seam differentiates the discrete mesh exactly (the FD
  column is the correctness gate).

The cone gates exercise every extension point at once: interior lateral
samples through the lift-bridge (`ConeAngle` / `Translate` duals), cap-rim
`Boundary` nodes and seam topology vertices through the cone implicit row, and
the reverse pullback through the new `cone_angle` cotangent slot. The torus
gates drive 575 lift-bridge nodes plus one topology vertex per build, and
price the `torus_major` / `torus_minor` slots independently.

## Regression

`cargo test -p vcad-kernel-diff -p vcad-kernel-tessellate -p vcad-kernel-geom`
green (M0–M5 suites unchanged — the refactors are behavior-preserving);
`cargo clippy … -D warnings` and `cargo fmt --check` clean.
