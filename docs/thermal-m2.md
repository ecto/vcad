# Heat-conduction FEA M2: the adjoint — gradients of a smoothed T_max

Third rung of the `vcad-kernel-thermal` ladder (M0/M1: `docs/thermal-m0.md`,
`docs/thermal-m1.md`). The payoff of building on an SPD operator: the
conduction matrix is **literally self-adjoint**, so the gradient of any
scalar objective costs one extra linear solve with the *same* operator and
the same PCG — the exact trick `vcad-kernel-particle` used for its Poisson
adjoint, without even needing that crate's radial-weight symmetrization.

## The objective, honestly

A hard max is non-differentiable: at a tie the gradient jumps between
voxels and an optimizer chasing it chatters. `adjoint::smooth_max_gradient`
optimizes the p-norm of the temperature **excess** over a reference,

> J = T_ref + ( Σ_v (T_v − T_ref)₊ᵖ )^(1/p)

over free voxels (pinned reservoirs are boundary data, not design
outcomes). The exponent (default p = 16) is a documented trade with a
checkable bracket: max ≤ J − T_ref ≤ max·N_active^(1/p). On the check
model below: hard max excess 62.9 K, smoothed 85.5 K, bracket ceiling
1.51× (N_active = 720) — honored, and both numbers are reported so nobody
mistakes the smoothed value for the physical peak. Powers are computed on
max-normalized excess, so no overflow at any p; sub-reference voxels
contribute nothing (positive part, stated).

## The gradients

One adjoint solve `A·λ = ∂J/∂T`, then chain rules into every parameter
that changes A or b **without moving the material mask**:

- **Per-source power**: dJ/dP = mean of λ over the source's voxels.
- **Per-region, per-axis conductivity**: through the harmonic-face
  derivative dG/dk_side = G²·(d/2k²_side)/A on every internal pair face and
  boundary half-cell the region owns.
- **Per-slot film coefficient** (six domain faces + the exposed rule):
  dG/dh = G²/(A·h²) on every link the slot created.

Geometry parameters (shape dimensions) move the discrete voxel mask; no
smooth adjoint covers that jump, so geometry stays finite-difference until
a shape-adjoint milestone — stated, not smoothed over. (Same division the
particle crate drew: potentials/currents adjoint, electrode positions FD.)

## Validation

`cargo run --release -p vcad-kernel-thermal --example gradient_check` on a
two-material board (FR4-ish anisotropic plate [2, 2, 0.8], aluminum
spreader k = 160, 3 W die, films h = 12/8, one 20 °C reservoir strip):

| parameter | adjoint | central FD | rel err |
|---|---:|---:|---:|
| dJ/dP (die) | 2.975124e1 | 2.975124e1 | 2.1e-9 |
| dJ/dk (plate, x) | −2.366801e1 | −2.366802e1 | 4.0e-7 |
| dJ/dk (plate, z) | −3.642413e-1 | −3.642418e-1 | 1.5e-6 |
| dJ/dk (spreader, y) | −9.879118e-4 | −9.879134e-4 | 1.7e-6 |
| dJ/dh (bottom) | −1.557541e0 | −1.557541e0 | 6.7e-8 |
| dJ/dh (top) | −1.577339e0 | −1.577339e0 | 7.0e-8 |

Five orders of magnitude of gradient scale, every sign physical (more
power hotter; more conductivity or film cooler), 96 forward + 94 adjoint
CG iterations for the entire table's adjoint column — the FD column costs
two full solves *per parameter*, which is the price argument for adjoints
in one table.

The paid-for lessons are encoded in the tests (`adjoint::tests`):

- **Frozen discretization**: k/h/P perturbations never re-voxelize (the
  mask depends only on shapes), so FD probes compare like with like — by
  construction here, by explicit reference-drop freezing in the particle
  crate. Stated in both.
- **Solver noise below probe resolution**: FD tests run CG at 1e-12, not
  the 1e-8 default — stopping noise must sit far below the difference
  being measured.
- **Degenerate-case honesty**: everything strictly below reference is a
  zero-gradient *result* (positive part clips), not an error; measuring
  against the ambient itself legitimately leaves ~1e-13 K of CG rounding
  excess, which is why the zero-gradient test measures against a reference
  strictly above the field.

## What this buys

`d_conductivity` is the seed of copper-pour/heatsink placement
optimization; `d_film` prices "would a fan help more than a thicker
plane"; `d_source_power` is the derating curve. M3 exposes these through
the named-parameter spec (adjoint-vs-FD roles per name), M4 puts the
objective value on the receipt with its bracket.

All previous caveats stand: conduction only, h supplied, no radiation.
