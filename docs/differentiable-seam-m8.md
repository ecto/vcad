# Differentiable seam — M8 design note

M3 built the mass-property QoIs and named the physics hook it was for; M5
built the adjoint that prices every parameter from one pullback. Both left the
same sentence in their notes: *the phyz coupling needs the sibling repo in the
session scope; the interface is frozen here and exercised by an analytic
functional.* M8 is that session. The real `phyz` and `loon` repos are cloned,
`vcad-kernel-physics` builds and passes against them, and this milestone closes
the loop the whole seam was built toward: **the gradient of a physics-rollout
objective with respect to a CAD parameter**, `dJ/dθ`, validated end to end
against a rebuild-and-resimulate finite difference.

## The factorization: geometry enters dynamics through ten scalars per body

For a **contact-free articulated rigid-body rollout**, CAD geometry reaches the
dynamics through exactly one channel. Featherstone ABA, the integrator, and any
objective read off the trajectory depend on the geometry of body *b* only
through its **mass properties**: mass `m`, center of mass `c`, and the inertia
tensor about the COM — ten scalars, `p_b = [m, c_x, c_y, c_z, I_xx, I_yy,
I_zz, I_xy, I_xz, I_yz]`. Nothing else about the mesh enters (contacts would;
see the boundary below). That is not an approximation — it is the structure of
rigid-body dynamics, and it gives an exact chain rule:

```text
dJ/dθ = Σ_bodies  ∂J/∂p_b · dp_b/dθ
```

Two factors, each computed the correct way for its side of the seam.

### `dp_b/dθ` — exact, from the seam

The differentiable seam already produces this. `mass_properties_with_derivative`
(M3) carries dual numbers through the polynomial mass-property integrals over
the frozen mesh: a per-parameter `ParamSeeding` (hand-written, or synthesized by
M6's `synthesize_seeding`) drives one seam pass, and every field's θ-derivative
falls out in that pass, to machine precision. No remeshing, no topology risk,
no combinatorics. This is the trusted half — the same pipeline M3 gated against
closed forms at 1e-12 and against FD at ~1e-9.

### `∂J/∂p_b` — central finite differences on the mass-property scalars

Here is the design decision the milestone turns on. The sensitivity of the
rollout objective to a body's ten mass-property scalars is computed by
**perturbing those scalars and re-running the rollout** — central differences,
≈20 rollouts per body (10 scalars × 2). This is the path the M3 note named as
the fallback and called cheap, and it earns the name: no CAD rebuild, no
re-tessellation, no boolean or fillet combinatorics under perturbation, no
chance of a topology flip. Only the physics integrator re-runs, on a body whose
inertia scalar moved by a hair. The trajectory is a smooth function of those
scalars, so the FD is clean.

#### Why not a phyz adjoint

phyz **does** ship a differentiation crate — `phyz-diff` — so this was probed
before defaulting to FD, per the brief. What it offers is the Jacobian of a
single dynamics step with respect to **state and control**:
`∂(q', v')/∂(q, v, ctrl)`, both finite-difference and (partially) symbolic.
That is the machinery you assemble a trajectory adjoint *through the state*
from. It is **not** a derivative with respect to **model inertia parameters**:
`Body::inertia` is baked into the `Model`, outside the `(q, v, ctrl)`
`phyz-diff` differentiates. The factor M8 needs — `∂J/∂p_b` — is not exposed by
phyz at the vendored version (0.3.1). So the live, and only, path for `∂J/∂p_b`
is central FD on the mass-property scalars.

This is a clean seam, not a compromise. If a future phyz grows a
parameter-adjoint (`∂J/∂inertia`), it drops in behind the *same* factorization:
replace the FD loop in `rollout_gradient` with the analytic `∂J/∂p_b` and the
chain rule, the seam's `dp_b/dθ`, and the optimizer above it are all unchanged.
The factorization is the contribution; which routine fills the physics factor
is an implementation detail it isolates.

## What landed

### The adapter — `vcad_kernel_physics::diff`

`rollout_gradient(bodies, rollout, θ, fd) -> Result<(J, dJ/dθ), DiffError>`.

- `bodies: &[DiffBody]` — each a CAD-parametric rigid body: a `build(θ)`
  B-rep function, a `seeding_for(brep, θ, k)` source (hand-written, or a
  one-liner over `synthesize_seeding`), a density, and tessellation params.
- `rollout: Fn(&[BodyMassProps]) -> f64` — builds a **contact-free** phyz
  model from the per-body mass properties, simulates a fixed trajectory, and
  returns the scalar objective. Deterministic, pure in its input.

Per call: one frozen-plan capture and one positions-only seam pass per body for
the nominal properties; `n·10·2` rollouts for the per-body `∂J/∂p` (independent
of the θ dimension); one seam pass per (body, parameter) for the exact
`dp/dθ`. Adding a CAD parameter costs one extra seam pass per body and nothing
in the rollout budget.

The return shape — a value and a `Vec<f64>` gradient behind a `Result` on seam
errors — is deliberately identical to the seam's own `objective_gradient`, so a
projected-GD or L-BFGS driver treats it as a black-box oracle. `BodyMassProps`
converts seam units (mm, `density·1e-9`) to SI (kg, m, kg·m²) and builds a
`phyz::SpatialInertia` (COM-frame inertia + COM offset — exactly the ten
scalars). The unit factors — `1e-3` on the centroid, `1e-6` on the inertia —
carry through the derivative unchanged, since θ is a length.

### The FD-step policy — `MassPropFdSteps`

Central differences trade `O(h²)` truncation against `O(ε·|J|/h)` roundoff. The
defaults sit near the `~1e-5`-relative sweet spot for a smooth `f64` rollout:
mass and inertia use a **relative** step (scaled to the body's own magnitude,
the inertia to its reference diagonal); the COM uses an **absolute** step,
because a centered body's COM is ~0 where a relative step would collapse.

## Gates (`tests/m8_rollout_gradient.rs`, `tests/m8_spinup_demo.rs`)

Two articulated models, both built from a single parametric CAD cylinder
(radius `θ = r`, axis Z) on a revolute joint:

- **Flywheel spin-up.** COM on the spin axis (gravity exerts no moment), spun
  from rest by a constant torque τ. `J = ω(T)`, the final angular speed. With
  constant torque and inertia, semi-implicit Euler gives `ω(T) = τT/I_zz`
  exactly, so this **isolates the inertia channel**: only `I_zz` of the ten
  scalars has nonzero `∂J/∂p`.
- **Gravity pendulum.** The same cylinder mounted with its axis rotated to +X,
  COM off the revolute (Z) axis, released from rest and swung under gravity.
  `J = q(T)`, the joint angle after T. The gravity torque scales with **mass**
  and **COM lever arm**; the swing rate with **inertia about the pivot** — all
  three channels enter.

| Gate | Model | Measured | Bound |
|---|---|---|---|
| `dp/dθ` in isolation (seam vs CAD-rebuild FD) | cylinder | `dI/dr` rel **~1.7e-10**, `dm/dr` exact | 1e-6 |
| End-to-end `dJ/dr` (adapter vs rebuild-and-resim FD) | flywheel | **4.99e-8** (`ω = 59.1`, `dω/dr = −23.654`) | 1e-4 |
| End-to-end `dJ/dr` (adapter vs rebuild-and-resim FD) | pendulum | **8.23e-10** (`dq/dr = −0.22795`) | 1e-4 |
| Determinism (two identical rollouts) | both | **bit-identical** | — |
| Synthesized vs hand-written seeding | flywheel | rel **< 1e-4** | — |

The end-to-end FD is the honest test the brief asks for: it rebuilds the CAD at
`r ± h`, re-derives the mass properties from the fresh mesh, and re-simulates —
the whole chain, differentiated numerically, against the adapter's analytic
`dp/dθ` fed through the FD `∂J/∂p`.

### The FD-noise analysis behind the 1e-4 bound

The gate tolerance is set at 1e-4 because a rollout FD is, in general, noisier
than a geometry FD — but on these models the measured agreement is far tighter
(5e-8 and 8e-10), and it is worth being precise about why, so the bound is
honest rather than lucky.

Three error sources stack into the comparison:

1. **The adapter's inner `∂J/∂p` FD** — relative step `~1e-5`, so `O(h²)`
   truncation `~1e-10` relative, roundoff `ε·|J|/h ~ 1e-11` absolute. This is
   the adapter's own noise floor.
2. **The reference end-to-end `∂J/∂θ` FD** — outer step `h_θ = 1e-3` mm on
   `r = 10` mm. The mass properties are smooth in `r` with no topology change
   (a cylinder stays a cylinder), so the mesh integral is `O(h_θ²)`-accurate
   just as the M3 isolation gate is; truncation `~1e-6` relative, roundoff
   negligible.
3. **The seam's `dp/dθ`** — exact (dual numbers), contributing nothing.

The flywheel's `ω(T) = τT/I_zz` is *linear* in `1/I_zz`, so both sides are
differentiating the same near-linear map through `I_zz(r)`; the residual is set
by source 1 and lands at 5e-8. The pendulum's swing is nonlinear but the short
horizon (`T = 0.15 s`, well before the bob swings far) keeps it smooth, and it
lands at 8e-10. Neither model touches a topology wall — the CAD is a lone
cylinder — so the M4/M9 fillet-correspondence noise (which forced M9's FD
spot-check up to `~1e-4`) is simply absent here. The 1e-4 bound is the honest
ceiling for a rollout FD on a smooth model; the models clear it by four to six
orders because the objectives are smooth and the geometry has no seam to graze.

### The demo — gradient descent through the full chain

`recover_radius_hitting_target_spin_speed`: a torque-driven disc must spin up to
a **target speed** `ω*` — the speed of the true radius `r* = 12 mm` — after a
fixed time under a fixed torque. Objective `J(r) = (ω(r) − ω*)²`, gradient
`dJ/dr = 2(ω − ω*)·dω/dr` flowing entirely through the M8 factorization.
Projected gradient descent with backtracking, driving `rollout_gradient` as a
black-box oracle, from `r₀ = 8 mm`:

**Recovered `r = 12.000000 mm` in 27 accepted iterations, `J = 1.2e-20`** —
`|r − r*| < 1e-8 mm`, and the achieved spin matches the target to `< 1e-5`
relative. Geometry driven by the gradient of a simulation. That is the
milestone.

The demo also proves `seeding_for` accepts `synthesize_seeding` naturally
(`|_brep, θ, k| synthesize_seeding(&build, θ, k, h)`): the machine-derived
seeding drives the same `dω/dr` as the hand-written one to `< 1e-4`.

## Notes and boundaries

- **Contact-free contract.** The factorization is exact *because* geometry
  reaches the dynamics solely through mass properties. The moment collision
  geometry participates — a contact, a joint limit bottoming out under load, a
  ground penalty — contact forces depend on the *surface*, a channel this
  gradient does not see, and the returned `dJ/dθ` is silently incomplete. The
  adapter's contract requires the rollout closure to build a contact-free
  model; it is documented on the module and on `rollout_gradient`, and the demo
  and gate models carry no colliders. The extension is real and known: the M5
  pullback (`evaluate_with_pullback`) already takes `∂J/∂x` on mesh nodes
  directly, so a contact adjoint that produces `∂J/∂x` on the collision surface
  prices every CAD parameter through the *same* pullback — the mass-property
  factorization is the smooth core, the surface pullback the contact skin. Not
  built here; the seam for it is.
- **Mass-property channel only.** If θ also moves a **joint anchor / mount
  frame**, that sensitivity is not included — the rollout applies mounts as
  fixed transforms. The same FD-on-scalars pattern extends to anchor
  coordinates when a model needs it; the factorization already sums over an
  arbitrary per-body scalar set, so anchor coordinates slot in as more scalars.
- **Multi-body.** `rollout_gradient` sums over `bodies`; the gates use one, but
  the loop and the `∂J/∂p`-per-body FD are written for the general case, and
  the reverse-mode seam (M5) is what keeps `dp/dθ` cheap when many bodies each
  carry many parameters.
- **phyz version.** The first build against the real siblings bumps
  `Cargo.lock` (phyz 0.3.0 → 0.3.1, loon-lang 0.5.0 → 0.7.0); committed with the
  milestone.
