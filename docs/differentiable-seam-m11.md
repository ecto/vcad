# Differentiable seam — M11 design note: the phyz-side adjoints

M8 shipped the physics rollout gradient with one inexact factor and one
honest boundary, and named both. The factor: `∂J/∂p` (sensitivity of the
rollout objective to each body's ten mass-property scalars) came from central
finite differences — ≈20 re-simulations per body — because phyz exposed no
inertia-parameter sensitivity. The boundary: **contact-free only**, because
producing `∂J_dyn/∂x` on a collision surface needed a phyz contact adjoint
that did not exist; the closeout note carried both as "phyz lives in its own
repository, outside this session's scope." M11 is the session with phyz in
scope. Both items are closed, on both sides of the repo boundary.

## What phyz gained: `phyz::diff`, an exact trajectory adjoint

New module in the umbrella `phyz` crate (mirrored into the split `phyz-diff`
crate as `phyz_diff::rollout`, per the tree's umbrella/split duplication
convention). One backward pass over a semi-implicit Euler rollout with a
final-state objective `J = g(q_T, v_T)` returns **both** sensitivities the
CAD seam needs:

- **`dJ/dπ`** per body — π = `[m, cx, cy, cz, Ixx, Iyy, Izz, Ixy, Ixz, Iyz]`
  (COM-frame inertia, body-frame COM), deliberately packed in the same order
  as vcad's `BodyMassProps::scalars()` so the gradient drops into the M8
  factorization with no re-indexing.
- **`∂J/∂x`** per collision-mesh vertex (body frame) — the contact cotangent,
  when ground-contact forces act during the rollout.

### Reverse over the trajectory, tangent within the step

The driver is a discrete adjoint: `λ_t = ∂J/∂x_t` backpropagates through the
step Jacobians. Semi-implicit Euler (`v' = v + dt·a`, `q' = q + dt·v'`,
`a = ABA(q, v, u; π, V)`) gives, with `w := dt·λ_q' + λ_v'`:

```text
λ_q = λ_q' + dt·(∂a/∂q)ᵀw        λ_v = w + dt·(∂a/∂v)ᵀw
dJ/dπ += dt·wᵀ·∂a/∂π            dJ/dV += dt·wᵀ·∂a/∂V
```

The within-step Jacobian columns are **exact**, not FD: tang's spatial
algebra is generic over `tang::Scalar`, so ABA + forward kinematics + the
contact law were written once over `T: Scalar` (the same move `phyz-diff`'s
symbolic tracer made with `ExprId`) and instantiated at `tang::Dual<f64>`.
One seeded dual lane per column; comparisons and clamps branch on the primal,
so the tangent is the derivative of the branch the `f64` rollout actually
takes.

The π lanes seed the ten inertia scalars through the symmetric packing (one
off-diagonal scalar seeds both matrix entries — the same convention a central
difference on the packed scalars probes), so `dJ/dπ` is directly the
derivative vcad's FD loop was estimating.

### The vertex channel avoids a lane per vertex

Vertices reach the dynamics only through each body's 6-component contact
wrench. So instead of `3·N` dual lanes, the driver prices the wrench
cotangent `χ_b = dt·wᵀ·∂a/∂(wrench_b)` once per contacting body (6 lanes
through the full step, seeded on an additive external-wrench input) and
contracts it against each vertex's **local** wrench Jacobian — 3 dual
evaluations of just the penalty law, holding the state at its stored nominal.
Cost per step: `O(nq + nv + 10·n_b + 6·n_b)` full dual steps plus `O(3·N)`
local evaluations. Everything is plain `f64` arithmetic in a fixed order —
two runs are bit-identical.

### The contact model is a differentiable forward model of its own

The adjoint does **not** differentiate the GJK/EPA production pipeline (a
single deepest-point contact whose location is a combinatorial function of
the mesh). Contact in `phyz::diff` is the standard differentiable-simulation
choice: every collision-mesh vertex below the ground plane contributes an
independent penalty wrench, `f_z = max(0, k·depth − c·v_z)` applied at the
vertex. The force is a smooth function of the vertex position wherever the
contact is active, the "contact set" is implicit in the smooth clamp (no
fixed-contact-set contract needed — better than the v1 contract the brief
allowed), and the gradient is the exact derivative of *this* forward model.
Friction is deliberately absent: its `‖v_t‖` kink sits exactly at the
sticking state a resting gate converges to.

### phyz-side gates (`crates/phyz/tests/diff_adjoint.rs`)

- **Flywheel closed form, 1e-12** (measured ~1e-15): semi-implicit Euler
  gives `v_T = steps·dt·τ/I_zz` exactly, so `dJ/dI_zz = −steps·dt·τ/I_zz²`
  is an exact discrete closed form; the other nine scalars are asserted
  structurally dead.
- **Gravity pendulum, all 10 π-scalars vs central FD at 1e-6**, with a
  lopsided body (COM offset on all axes, dense inertia) and ≥4 channels
  asserted live (mass, COM x/y, I_zz).
- **Two-link 3D chain (mixed joint axes), all 20 π-scalars vs FD at 1e-6**,
  ≥14 live — this is the gate that exercises the articulated-inertia
  propagation a single body never touches. (The FD oracle's inertia step is
  1e-6, not 1e-7: at 1e-7 the oracle's own roundoff sits at the gate —
  measured rel 1.01e-6 — which is FD noise, not adjoint error.)
- **Box settling on the plane, vertex gradient vs FD at 1e-4**, released at
  rest exactly touching (the whole trajectory stays on one smooth branch),
  plus closed-form anchors: `∂q_T/∂z = −1/4` per bottom vertex at
  equilibrium (load shared by 4 vertices), top/tangential channels exactly
  dead.
- **Tilting paddle** (box offset on a revolute joint, resting tilted):
  the same FD gate with a live rotation — frame conventions, torque arm,
  rotational vertex velocity — including the x-channel that only exists
  under tilt.
- **Determinism**: bit-identical outputs across runs.

### Contract (both repos)

Single-DOF joints (revolute/prismatic) + fixed — the same domain as the
symbolic tracer, and enough for every gate model the seam program has used;
multi-DOF joints panic rather than misdifferentiate. Open-loop control (a
state-feedback law would add `∂u/∂x` terms the driver does not model).
Final-state objectives with caller-supplied analytic gradients.

## What vcad gained: both M8 factors analytic, and the boundary closed

### `rollout_gradient_adjoint` — the FD factor replaced (Task 1)

`AdjointRolloutSpec` exposes the rollout's structure (model builder, initial
state, open-loop control, objective + gradient); the adapter runs one seam
pass per body for nominal props, one phyz adjoint pass for the exact
`∂J/∂p`, and the usual per-(body, parameter) seam pass for the exact
`dp/dθ`. The factorization — and every downstream consumer — is unchanged;
`rollout_gradient` (FD) stays as the fallback for rollouts the spec cannot
express (opaque closures, state feedback, multi-DOF joints, or a phyz
without the adjoint).

One new contract, enforced with a runtime check: `build_model` must install
each body's inertia **verbatim** (`props[i].to_spatial_inertia()`, CAD body
frame). Mounting belongs in the joint (`parent_to_joint`, `axis`) — the M8
test's `si.transform(&mount)` idiom would silently decouple `∂J/∂p` from the
seam's `dp/dθ`. The M11 pendulum shows the compliant form: revolute about
body-X instead of a rotated inertia.

Gates (`m11_adjoint_rollout.rs`), on the M8 flywheel + pendulum re-expressed
as specs:

- adjoint vs the FD path at 1e-5 — measured 1.2e-10 (flywheel),
  4.9e-10 (pendulum); primal objectives agree to 1e-12 (same integrator,
  same trajectory);
- adjoint vs rebuild-and-resimulate end-to-end FD at 1e-4 (the M8 bar) —
  measured 5.0e-8 and 1.2e-9;
- determinism bit-identical.

### `contact_rollout_gradient` — the last honest boundary (Task 2)

The M8 contract said *contact-free only*, and the closeout note kept
"contact-adjoint dynamics" deferred. Closed:

```text
dJ/dθ = Σ ∂J/∂p·dp/dθ            (mass channel — adjoint · seam, both exact)
      + Σ pullback(∂J/∂x)·seeding (surface channel — contact adjoint · M5
                                    pullback, both exact)
```

Each skinned body collides with the ground through **its own frozen-plan
seam mesh** (mm → m), so the vertex cotangent the adjoint returns is already
indexed by plan node; scaled back to per-mm it goes through
`evaluate_with_pullback` exactly as `surface_gradient` promised a contact
adjoint would. No finite difference anywhere in the chain.

Gate model (`m11_contact_gradient.rs`): the CAD cylinder (radius θ) lying on
its side — a rotated mount frame on a vertical prismatic joint — resting on
the ground; `J = q(T)`, the settled height. Growing the radius moves the
contact line down the body (surface channel: the `y_body = r` vertex drops
by exactly dθ, so `+1e−3 m per mm`) *and* adds mass that sinks the body
deeper into the penalty spring (mass channel, negative, a few percent of the
total — measured `dJ/dr = 9.82e-4`, i.e. ~2% below the pure-surface `1e-3`) —
two live channels pulling in opposite directions. Gates:

- full chain vs rebuild-and-resimulate FD (CAD rebuild, re-tessellation,
  re-simulation with contact) at 1e-4 — measured `dJ/dr = 9.8202e-4` vs FD,
  rel **3.5e-7**;
- channel liveness: the gradient sits in the surface-dominated range, and
  doubling the density measurably lowers it (mass channel load-bearing);
- determinism bit-identical.

## Honest boundaries

- The contact adjoint differentiates **phyz's diff rollout** (per-vertex
  penalty against a ground plane, no friction), not the GJK/EPA production
  pipeline of `PhysicsWorld`. A vcad gym rollout is not automatically
  differentiable; the differentiable path is `contact_rollout_gradient`'s
  own forward model, and its FD gates rebuild and re-simulate *that* model.
- Single-DOF joints, open-loop control, final-state objectives (running
  costs would be a mechanical extension of the λ recursion — nothing needs
  them yet).
- Force discontinuities at contact-activation instants (impact with nonzero
  approach speed) are inherited from the penalty law's damping term: the
  adjoint returns the exact derivative of the discrete trajectory, but that
  derivative is only as meaningful as the trajectory is smooth in its
  parameters. The gates pin the settled/steady regime where the law is
  smooth; a bouncing objective would need a restitution-aware smoothing
  neither repo has.
- The FD path (`rollout_gradient`) stays, deliberately, for everything the
  structured spec cannot express.
