# Differentiable seam — closeout: the named follow-ups

M0–M10 (`differentiable-seam-m0-m2.md` … `-m10.md`) shipped the seam and its
first consumer. Each milestone's design note also named what it deliberately
did **not** build. This wave closes those follow-ups — every deferred item
that is reachable from this repository — and records the two that remain
deferred, with the reason.

## What this wave ships

### 1. Cone/torus second order (M10's only deliberate omission)

M10 restricted `evaluate_with_second_derivative` to plane/cylinder/sphere,
whose surface points are linear in their seeded fields. The restriction is
gone, in both places it lived:

- **Lift nodes** now go through a nested-dual surface lift
  (`lift_surface_second`, `Dual<Dual<f64>>` with every seeded field packed
  `((f, ḟ), (ḟ, f̈))`), so `ẍ = ∂x/∂field·field̈ + ∂²x/∂field²·fielḋ²` is
  exact for **every** surface kind — the cone's `tan α` and the torus radii
  nonlinearities included. The old two-first-order-lifts shortcut survives as
  the special case where the second term vanishes; a unit gate asserts the
  linear kinds still produce exactly zero acceleration under empty
  acceleration seeds.
- **Vertex/Boundary rows** (`constraint_row_2`) gained cone and torus
  velocity-curvature arms in closed form. Differentiating each implicit
  identity twice and keeping the terms quadratic in first-order rates:

  ```text
  cone:   curv = −2‖ẇ⊥‖² + τ̈ h² + 4 τ̇ h ḣ + 2 τ ḣ²,
          τ = tan²α, τ̇ = 2 tanα sec²α·α̇, τ̈|α̈=0 = 2 sec²α(sec²α + 2tan²α)·α̇²
  torus:  curv = −[ 2(ρ̇ − Ṙ)² + 2(ρ − R)(‖ẇ⊥‖² − ρ̇²)/ρ + 2ḣ² − 2ṙ² ]
  ```

  with `ẇ` the vertex rate relative to the apex/center. The torus middle term
  is the non-constant `∇²g` the M10 note called "the mechanical extension".
  `DiffError::SecondOrderUnsupported` no longer exists: every kind with an
  implicit form has a second-order form.

Gates: the fixed-apex cone's `d²V/dα²` against its **exact discrete closed
form** `(h·k/3)·C·(2sec⁴α + 4tan²α sec²α)` at rel ≤ 1e-9, with node
accelerations asserted **nonzero** (the first gate in the suite where the
acceleration machinery is live rather than proving zeros); torus `d²V/dr²`
against the continuum `4π²R` (5% polygonization band) and FD of the analytic
`dV/dr`; torus `d²V/dR²` = 0 exactly (V linear in R — the curvature terms
must cancel, and do); and an `r(θ) = θ²` chain-rule consistency gate at
1e-12 that exercises nonzero acceleration seeds through every torus node
class. Closed-form unit tests pin the new rows (cone rim: `ẍ = d·2sec²α tanα
α̇²·û`; torus equator nonlinear-field and major-radius-cancellation cases).

### 2. Physics adapter: anchor channel (M8 note)

M8's factorization note promised "anchor coordinates slot in as more
scalars". `rollout_gradient_with_anchors` delivers it:

```text
dJ/dθ = Σ_bodies ∂J/∂p·dp/dθ  +  Σ_anchors ∂J/∂a·da/dθ
```

`∂J/∂a` comes from the same central-FD-rollout pattern as `∂J/∂p` (no CAD
rebuild); `da/dθ` from a central difference of the caller's **anchor map**,
a pure function of θ with no geometry in it. `rollout_gradient` is now the
anchor-free special case of the same implementation. Gates: the anchor
channel in isolation (fixed geometry, pivot = θ) matches a full
rebuild-and-resimulate FD at 1e-6; combined channels (radius drives mass
props *and* a pivot at `2r`) at 1e-4, with the anchor contribution asserted
load-bearing.

### 3. Physics adapter: surface skin (M8 extension)

The M8 note's sentence — "the mass-property factorization is the smooth
core, the surface pullback the contact skin" — is now code:

- `surface_gradient(body, θ, ∂J/∂x)` is the raw skin: a mesh cotangent on
  the body's frozen-plan nodes in, per-parameter `dJ/dθ` out, via **one** M5
  pullback plus a contraction per parameter. This is the exact entry point a
  future phyz contact adjoint plugs into.
- `rollout_gradient_with_surface` composes it with the smooth core for
  objectives `J = J_dyn(mass props) + J_surf(surface nodes)` — ground
  clearance, penetration, or any other penalty that reads the tessellated
  surface. The surface channel is exact end to end (analytic node gradient →
  pullback → contraction; no FD anywhere in it).

Gates: the skin in isolation reproduces the N-gon prism closed form
`dV/dr = 2krh` at 1e-9 through both entry points; flywheel spin-up + a
radial surface penalty matches a rebuild-and-resimulate FD at 1e-4, with
both channels asserted live and summing linearly.

**Honest boundary, unchanged:** surface-dependent *dynamics* — contact
forces acting during the rollout — need `∂J_dyn/∂x` from a phyz-side
adjoint that does not exist at any vendored version. The skin is the seam
it drops into; until then the contact-free contract on `rollout_gradient`
stands.

### 4. Document parameter gradient (M6's rejected stretch, unlocked)

M6 rejected this as unreachable: the Rust document evaluator (`vcad-eval`)
sat behind the excluded `loon` sibling. With the sibling in scope,
`vcad_eval::diff::document_parameter_gradient` closes the product-level
loop: a `.vcad` document's **named parameter** (the existing `parameters` +
`bindings` sidecar) differentiates end to end —

```text
parameter → resolve bindings → evaluate_document → BRep per part
          → synthesize_seeding (M6) → frozen-plan seam
          → d(volume, mass, centroid, inertia)/dθ
```

— per solid part, with zero hand seeding. Probe evaluations are
pre-validated (part count must hold at θ ± h), so a parameter value that
crosses a part-count boundary errors instead of returning a wrong gradient.
Gates: a parametric IR cylinder (radius bound to `"r"`) hits the N-gon
closed form `dV/dr = 2krh` at 1e-9, mass scales by density, the centroid
derivative vanishes, and `dI_zz/dr` matches a document-rebuild FD at 1e-6;
a **loon-authored** document (`vcad_loon::eval_vcad` → bind → differentiate)
reaches the same closed form — the exact path the M6 note deferred.

### 5. Fillet miter corners (the per-edge fillet fix's named follow-up)

Filleting **adjacent** edges — edges sharing a vertex — now works through
the correct subset builder instead of falling back to the legacy
inset-everything pipeline. The key geometric fact: at a vertex where two
equal-radius blends meet over a shared face `S` with equal dihedral angles
against `S` (every prism-like corner), the two blend cylinders intersect
**exactly** in a planar curve on the bisector plane of the edge directions.
So the corner is a *miter*, not a spherical patch:

- both trimmed cylinder ends reference the **same** sampled curve, making
  the weld exact by construction;
- the shared face's corner collapses to the curve's end on `S`;
- the two other faces corner at the opposite end, which lies on the
  surviving third edge of the trihedral corner.

Open chains, closed rings (a box's top rim: four edges, four miters, no
caps), and the old independent sets all go through one generalized builder;
anything outside the symmetric-chain domain still falls back to the legacy
pipeline rather than producing wrong geometry.

The tessellator gained a matching **ruled two-chain path** for cylinder
faces whose end rings are angle-paired but not planar (the slanted miter
trims — the old rectangular `(u, v)` grid overshot them and left an open
seam). Detection is strict (exact angle pairing, non-constant `v`), so
only blend faces take the new path; constant-`v` faces keep the existing
grid bit for bit.

Gates: L-chain, U-chain, and the closed top rim on a cube match the exact
closed form `V = a³ − r²(1 − π/4)·Σlen + (5/3 − π/2)r³·(#corners)` to
~1e-6 — the corner constant is `∫₀ʳ (r − √(2rz − z²))² dz`, the overlap of
the two removed prisms — with structural watertightness asserted (every
mesh edge shared by exactly two triangles).

## What stays deferred, and why

- ~~**phyz inertia-parameter adjoint.**~~ **Closed in M11**
  (`differentiable-seam-m11.md`): `phyz::diff`'s trajectory adjoint computes
  `∂J/∂p` exactly; `rollout_gradient_adjoint` swaps it in behind the
  unchanged factorization, with the FD path kept as the fallback for
  rollouts the structured spec cannot express.
- ~~**Contact-adjoint dynamics.**~~ **Closed in M11**: the phyz contact
  adjoint produces `∂J_dyn/∂x` on the body's frozen-plan seam mesh under a
  differentiable per-vertex penalty contact model, and
  `contact_rollout_gradient` prices it through the `surface_gradient`
  pullback — the M8 "contact-free only" boundary no longer stands (for the
  diff rollout's own forward model; the GJK/EPA production pipeline remains
  non-differentiable).
- **Cone-tangent-to-plane tangency rows** (M7 note): still waiting for a
  gate model that produces one — consistent with `tangency_rows`'
  documented "unknown kind = no tangency information" contract.
- **Asymmetric / non-trihedral fillet corners** (unequal dihedral angles,
  ≥3 blends at a vertex outside `fillet_all_edges`): the bisector-plane
  identity that makes the miter exact does not hold there; those
  selections fall back to the legacy pipeline. A sphere-patch corner for
  the 3-blend case is the natural next construction.

## Validation

Full workspace suite (only the GTK-bound `vcad-desktop` excluded), clippy
`-D warnings`, `cargo fmt --check` — green at every commit of the wave.
