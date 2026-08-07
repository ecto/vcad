# Structural FEA — M4: the shape adjoint

`crates/vcad-kernel-fea/src/adjoint.rs` turns the forward solver from a
verdict into a **direction**. M0–M3 answer *will this bracket break*; M4
answers *which way do I move the geometry to fix it*, exactly — not by
finite-differencing the solver, but by solving the adjoint system once.

This is the structural link in pond's co-design chain (`POND.md` §4b). The
thesis needs a task-performance gradient that reaches geometry; a gradient
that reaches geometry without a stress constraint produces parts that are
confidently wrong. M4 is the constraint, and it is differentiable.

## Why it belongs here and not in phyz

Solid mechanics is physics, which argues for phyz. It lost to three
practical facts: the solver consumes BRep tessellation, material
assignment, and the receipt schema — all vcad's; its output feeds DFM and
design verdicts rather than simulation rollouts; and phyz's role in the
co-design loop is *upstream*, producing load cases (joint reactions from a
rollout) that this crate consumes. phyz never appears inside a static
solve. Being wrong costs a crate move plus load-case re-plumbing — days,
because the adjoint math and the seam contract are placement-independent.

The tang constraint is satisfied the house way. Nobody tapes through a
2000-iteration PCG loop; instead the crate exposes a **vector-Jacobian
product** — `dJ/d(node coordinates)` — that contracts with
`vcad-kernel-diff`'s tang-taped `dx/dθ`. Same pattern as
`vcad-kernel-thermal`'s one-extra-solve adjoint, `vcad-kernel-particle`'s
Poisson adjoint, and `phyz::diff`'s contact adjoint. The chain
`task → phyz → vcad → loon` stays unbroken because the seam is where the
tape lives, not the solver.

## What it computes

For `K(x)·u = f` and an objective `J(u, x)`:

```text
K·λ = ∂J/∂u,      dJ/dx = ∂J/∂x|_u − λᵀ·(∂K/∂x)·u
```

`K` is symmetric, so the adjoint solve reuses the forward operator, the
forward preconditioner, and the same PCG — **one extra solve**, regardless
of how many parameters the design has. That asymmetry is the whole point:
finite differences cost one solve *per parameter*, so a 200-parameter part
costs 400 solves for a worse answer.

The element shape derivatives are closed-form and, pleasingly, symmetric:

```text
∂g_{m,p} / ∂x_{k,j}  =  − g_{k,p} · g_{m,j}
∂V       / ∂x_{k,j}  =    V · g_{k,j}
```

both valid for all four nodes including node 0 (via `Σ_i g_i = 0`). With
`H_v = Σ_i v_i ⊗ g_i` and `σ(·)` the unit-E stress operator, an element's
contribution to `λᵀKu` is `V·σ(H_u):H_λ`, whose derivative gives the whole
per-node gradient in **one pass over the elements** — no assembled matrix,
no per-parameter re-solve.

### Objectives (`Qoi`)

| QoI | Units | Adjoint | `∂J/∂x|_u` |
|---|---|---|---|
| `Compliance` | N·mm | self-adjoint, `λ = u/E`, **no solve** | 0 |
| `MeanDisplacement { region, direction }` | mm | one solve | 0 |
| `SmoothMaxVonMises { p, threshold_mpa }` | MPa | one solve | non-zero |

A hard max is not differentiable — at a tie the gradient jumps between
elements and an optimizer chasing it chatters — so peak stress gets the
thermal crate's treatment, a p-norm of the excess over a threshold τ:

```text
J = τ + ( Σ_e (vm_e − τ)₊ᵖ )^(1/p),   max ≤ J ≤ τ + (max − τ)·N_active^(1/p)
```

normalized by the peak excess so no `p` overflows. Both `J` and the hard
max are reported so the bracket is always checkable.

**The threshold is not a nicety — it is what makes the objective usable.**
With τ = 0 every stressed element is active, so `N_active` is the entire
element count and the bracket is worthless in practice. Measured on the
80-cell cantilever:

| p = 8 | J (MPa) | true peak | over-read | active elements |
|---|---|---|---|---|
| no threshold | 150.21 | 71.97 | **+108.7 %** | 38 400 |
| τ = 55 MPa | 75.62 | 71.97 | **+5.1 %** | 897 |

A constraint that reads twice the true peak rejects safe designs. Set τ
near the stress that actually matters (a fraction of yield). The
unthresholded norm is not *wrong* — it just answers a question about the
whole stress field rather than about the peak.

## Scope: fixed topology, frozen discretization

The parameter enters through **node coordinates of a frozen
discretization**. Everything discrete is held fixed: tet connectivity,
which nodes a region selects, which elements exist. This is scoped
deliberately, and the reasons are load-bearing:

- **`mesh::tet_fill` is not differentiable.** It re-voxelizes per geometry;
  a parameter sweep pops whole tets in and out of existence and `J` is a
  step function of the parameter at those events. Re-meshing per optimizer
  step and finite-differencing across it measures the staircase, not the
  physics. **A consumer must mesh once and supply node velocities
  `dx/dθ`** — precisely what `vcad-kernel-diff`'s frozen-tessellation seam
  produces. `ShapeGradient::contract(&velocity)` is that interface.
- **Region membership is discrete.** A parameter that drags a node across a
  load or support boundary changes the model, not just its geometry. Pad
  regions clear of the nodes they mean to catch.
- **Loads are prescribed nodal forces**, so `∂f/∂x = 0`. A pressure or body
  load would make that term live; it is not implemented.

Exact predicates exist to make discrete decisions robust. Those decisions
are not differentiable, and co-design does not need them to be.

## Validation

Every gradient is checked against central finite differences **on the same
frozen connectivity** (`adjoint::tests`, reproduced by
`examples/gradient_report.rs`), with PCG tightened to 1e-12 so solver
stopping noise sits far below the FD signal. Two velocity fields per QoI: a
uniform thickness scale, and a taper whose magnitude varies along the beam
(which exercises the full per-node gradient rather than one global scale).

Relative error, adjoint vs central FD:

| QoI | thickness | taper |
|---|---|---|
| compliance | 2.1e-9 | 2.1e-9 |
| tip deflection | 2.1e-9 | 2.1e-9 |
| smooth-max stress (τ = 0) | 1.9e-9 | 6.5e-9 |
| smooth-max stress (τ = 55) | 2.5e-9 | 3.2e-8 |

(80-cell mesh, 8019 nodes / 38 400 tets; the 40-cell numbers are the same
order.) These sit at the finite-difference truncation floor — the adjoint
is exact on the discrete model, which is the property an optimizer needs.

**The checks can fail.** Three deliberate mutations of the adjoint were
each caught:

| mutation | caught by | error |
|---|---|---|
| drop the explicit `∂J/∂x` term | stress tests only | 3.4 % |
| drop the test-function term | all four FD tests | 33–49 % |
| transpose the stress/gradient pairing | all five gradient tests | 10³–10⁶ × |

The selectivity of the first is the useful signal: it perturbs only the
objective that has an explicit shape term, exactly as the math predicts.

A closed-form check guards the layer finite differences cannot reach — FD
proves the adjoint matches the *discrete model*, not that the model is
right. Beam theory gives `d(deflection)/dt = −3·δ_bend/t`; the discrete
gradient converges onto it monotonically under refinement (−0.198 at h = 2
mm, −0.211 at h = 1 mm, against −0.217 exact), the residual gap tracking
the constant-strain-tet bending stiffness that the forward solve already
reports.

Cross-checks that also hold: the compliance gradient equals `P ×` the tip
deflection gradient for the single-load case (to 1e-8); the self-adjoint
shortcut agrees with the general path; the p-norm brackets the hard max and
tightens monotonically in `p`; the reported `Solution` is the one that was
differentiated, not a second solve.

## Fail-closed behavior

- Smoothing exponent must be finite and > 1 → `InvalidSmoothingExponent`.
- Threshold must be finite and ≥ 0 → `InvalidThreshold`.
- An objective region selecting no node → `EmptyQoiRegion`.
- A zero-length direction → `ZeroDirection`.
- A stress field that is identically zero with τ = 0 → `ZeroStressField`
  (the model has no load path; reporting a zero gradient would hide that).
- Stress entirely below a positive τ is **not** an error — it is a
  satisfied constraint: `J = τ`, zero gradient, and the true peak still
  reported so the caller can see the headroom.
- A velocity field of the wrong length panics: that is a programming error
  (a field from a different mesh), not a runtime condition.

## Not in scope for M4

In rough order of value:

- **Seam registration with `vcad-kernel-diff`** — the velocity fields are
  currently the caller's to supply. Wiring `document_parameter_gradient` so
  a named `.vcad` parameter produces `dx/dθ` on the tet mesh is the next
  chunk, and it is what actually closes the loop to loon.
- **WASM binding + MCP surface** (`analyze_structure` returning gradients,
  a `structural_gradient` tool), and receipt claims for gradient-derived
  quantities.
- **phyz load-case ingestion** — joint reactions from a rollout as
  `FeaSpec` loads. This is what makes the capability load-bearing for
  co-design rather than merely available.
- **Boundary-conforming mesh**, which removes the staircase and would make
  the p-norm need volume weights (it is currently unweighted, exact for the
  uniform Kuhn lattice).
- Multi-load-case aggregation, buckling, and second-order (Hessian)
  information for a Newton-type optimizer.
